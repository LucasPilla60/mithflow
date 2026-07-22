//! El hilo que decide. Es el único que escribe el estado.
//!
//! Todo lo que puede cambiar el rumbo de la aplicación —el atajo, la bandeja,
//! los comandos del frontend, el motor— manda un [`Mensaje`] por el mismo canal
//! y espera. Acá no hay locks sobre el estado ni carreras posibles: hay una
//! cola y un solo lector.
//!
//! # La transición que importa
//!
//! Al pasar de `Grabando` a `Transcribiendo`, el `Vec<f32>` con el audio **se
//! mueve** al motor. El director se queda literalmente sin el buffer, así que
//! un segundo atajo apretado durante la transcripción no tiene nada que pisar.
//! La versión Python tenía ahí un bug real (dos funciones escribiendo el mismo
//! buffer global); acá el compilador no deja escribirlo.

use crate::ajustes::{self, Ajustes};
use crate::bandeja;
use crate::estado::{Estado, EstadoCompartido, EstadoDto, FalloDelMotor};
use crate::motor::{self, AlMotor};
use crate::sonidos::{Sonidos, Tono};
use crate::superpuesta::{self, NivelAudio};
use crate::{atajo, eventos};
use mithflow_core::audio::Recorder;
use mithflow_core::{config, history, DictationResult};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;
use tauri::AppHandle;

/// Cada cuánto se despierta el director mientras graba, para vigilar el tope de
/// duración. 250 ms no se notan y no cuestan nada: el hilo duerme.
const LATIDO: Duration = Duration::from_millis(250);

/// El latido cuando además hay que alimentar el medidor de la ventanita.
///
/// 40 ms son 25 cuadros por segundo: suficiente para que las barras se muevan
/// como el habla y no como un semáforo, y lejos todavía de inundar el canal de
/// eventos. Cada despertar es leer tres átomos y emitir un objeto de siete
/// números; el hilo duerme el resto del tiempo.
///
/// **Rige sólo mientras se graba y con el indicador activado**: sin él el
/// director sigue latiendo cada [`LATIDO`], como siempre.
const LATIDO_CON_MEDIDOR: Duration = Duration::from_millis(40);

/// Lo que el resto del programa le puede pedir al director.
pub enum Mensaje {
    /// Se apretó el atajo.
    Pulso,
    /// `true` pausa, `false` reanuda.
    Pausa(bool),
    AlternarPausa,
    /// El motor terminó de cargar y calentar: `Ok` con un resumen, `Err` con el
    /// motivo por el que no se puede dictar **y de qué clase es** — que todavía
    /// no haya modelo descargado no es lo mismo que uno que no carga.
    MotorListo(Result<String, FalloDelMotor>),
    /// Resultado de un dictado, y la entrada que quedó en el historial.
    ///
    /// `salida` va en `Box` por la misma razón que [`Mensaje::Ajustados`]: un
    /// enum mide lo que su variante mayor, y `DictationResult` (dos textos
    /// completos más la entrada del historial) es varias veces más grande que
    /// todo lo demás que pasa por esta cola. Sin el `Box`, cada `Pulso` del
    /// atajo arrastraría ese tamaño.
    Transcripcion {
        salida: Box<Result<Option<DictationResult>, String>>,
        entrada: Option<history::Entry>,
    },
    /// No se pudo enganchar el teclado.
    AtajoRoto(String),
    /// El usuario guardó Ajustes. El director aplica lo suyo (el tope de
    /// grabación) y le reenvía al motor lo que es del dictado.
    ///
    /// Pasa por acá y no directo del comando al motor por la misma razón que
    /// todo lo demás: el director es el único que le habla al motor, así que no
    /// hay dos escritores compitiendo por esa cola.
    Ajustados(Box<Ajustes>),
    /// Terminó bien la descarga de un modelo.
    ///
    /// El hilo de la descarga no decide nada: manda esto y sigue. **Quién sabe
    /// si hay un motor corriendo es el director y nadie más**, y esa pregunta no
    /// se puede contestar mirando un espejo desde otro hilo sin abrir una
    /// carrera — dos descargas seguidas leerían las dos "no hay motor" y
    /// lanzarían dos, con 1,5 GB de pesos cada uno. Acá hay una cola y un solo
    /// lector: la segunda ve lo que dejó la primera.
    ModeloDescargado,
}

/// El extremo del canal, para guardarlo en el estado de Tauri.
///
/// No necesita `Mutex`: `mpsc::Sender` es `Sync` desde Rust 1.72 y `send` toma
/// `&self`. Un test de este módulo lo comprueba, para que una regresión en esa
/// garantía se vea acá y no como un error de tipos indescifrable en `manage`.
pub struct AlDirector(Sender<Mensaje>);

impl AlDirector {
    pub fn nuevo(emisor: Sender<Mensaje>) -> Self {
        Self(emisor)
    }

    /// Manda un mensaje. Si el director murió no hay nada que hacer desde acá:
    /// se registra y se sigue, porque quien llama suele ser un clic de menú.
    pub fn enviar(&self, mensaje: Mensaje) {
        if self.0.send(mensaje).is_err() {
            eprintln!("el director no está escuchando; se descarta el mensaje");
        }
    }

    /// Un emisor suelto, para los hilos que arranca un comando y que viven más
    /// que el `invoke` que los creó. Un `State` no se puede mover a otro hilo;
    /// un `Sender` sí, y clonarlo no cuesta nada.
    pub fn emisor(&self) -> Sender<Mensaje> {
        self.0.clone()
    }
}

/// Arranca el director en su propio hilo.
///
/// `al_director` es un emisor **hacia el propio director**: lo necesita para
/// dárselo a un motor arrancado en caliente, que le va a contestar por ahí. Que
/// el director retenga un emisor propio significa que su cola nunca queda
/// desconectada sola, lo que en la práctica no cambia nada: el atajo y el estado
/// de Tauri ya retienen los suyos durante toda la vida del proceso, y la
/// aplicación termina por "Salir" en la bandeja.
pub fn lanzar(
    app: AppHandle,
    al_director: Sender<Mensaje>,
    cola: Receiver<Mensaje>,
    motor: motor::Motor,
    espejo: Arc<EstadoCompartido>,
    sonidos: Sonidos,
    cfg: &Ajustes,
) {
    let limite_grabacion_s = cfg.limite_grabacion_s as f32;
    let modelo_elegido = cfg.modelo.clone();
    let indicador = cfg.indicador;
    let indicador_posicion = cfg.indicador_posicion.clone();
    let creado = std::thread::Builder::new()
        .name("mithflow-director".into())
        .spawn(move || {
            Director {
                app,
                al_director,
                estado: Estado::Cargando,
                pausado: false,
                grabadora: None,
                al_motor: motor.al_motor,
                modelo_en_el_motor: motor.modelo,
                modelo_elegido,
                espejo,
                sonidos,
                limite_grabacion_s,
                indicador,
                indicador_posicion,
            }
            .atender(cola)
        });
    if let Err(e) = creado {
        eprintln!("no pude crear el hilo del director: {e}");
    }
}

struct Director {
    app: AppHandle,
    /// Emisor hacia sí mismo, para el motor que se arranca en caliente.
    al_director: Sender<Mensaje>,
    estado: Estado,
    pausado: bool,
    /// Existe sólo mientras se graba. Que sea `Option` y no un campo siempre
    /// presente es lo que cierra el micrófono al terminar: el indicador de
    /// Windows se apaga y en una notebook eso es batería.
    grabadora: Option<Recorder>,
    al_motor: Sender<AlMotor>,
    /// El `.gguf` con el que se lanzó el motor, si se lanzó alguno.
    ///
    /// **Es la única información que hay para no mentirle al usuario cuando
    /// elige un modelo en Ajustes**: el estado dice si hay motor y qué está
    /// haciendo, pero no con qué archivo, y avisar "el modelo cambia la próxima
    /// vez que abras MithFlow" mientras el motor está cargando **ese mismo
    /// archivo** es negar lo que el usuario acaba de hacer. Ver
    /// [`aviso_al_cambiar_el_modelo`].
    modelo_en_el_motor: Option<PathBuf>,
    /// La clave de modelo que hay guardada en Ajustes (`"auto"`, `"F16"`…).
    /// Vive acá y no en el comando porque es el director quien puede decidir
    /// qué significa cambiarla.
    modelo_elegido: String,
    espejo: Arc<EstadoCompartido>,
    sonidos: Sonidos,
    /// Tope de duración de una grabación, configurable desde Ajustes.
    limite_grabacion_s: f32,
    /// Si la ventanita de grabación está activada. Vive acá —y no se relee de
    /// Ajustes en cada publicación— porque el director publica desde su hilo y
    /// abrir el archivo de ajustes en ese camino sería I/O adentro de la
    /// máquina de estados.
    indicador: bool,
    /// Una de `superpuesta::POSICIONES`.
    indicador_posicion: String,
}

impl Director {
    fn atender(mut self, cola: Receiver<Mensaje>) {
        // El primer estado ya está en el espejo, pero la bandeja todavía no lo
        // vio: se publica una vez al arrancar.
        self.publicar();

        loop {
            let mensaje = if matches!(self.estado, Estado::Grabando) {
                match cola.recv_timeout(self.latido()) {
                    Ok(m) => m,
                    Err(RecvTimeoutError::Timeout) => {
                        // El orden importa: primero se cuenta lo que está
                        // entrando y después se mira el tope, porque vigilarlo
                        // puede cortar la grabación y dejar de haber qué contar.
                        self.publicar_el_nivel();
                        self.vigilar_tope();
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            } else {
                match cola.recv() {
                    Ok(m) => m,
                    Err(_) => return,
                }
            };
            self.procesar(mensaje);
        }
    }

    /// Cada cuánto despertarse. Sólo se acelera cuando hay una ventanita
    /// esperando el medidor: quien desactivó el indicador sigue con los 250 ms
    /// de siempre.
    fn latido(&self) -> Duration {
        if superpuesta::hay_que_medir(self.estado.clave(), self.indicador) {
            LATIDO_CON_MEDIDOR
        } else {
            LATIDO
        }
    }

    /// Manda a la ventanita cuánto está entrando por el micrófono.
    ///
    /// Es lo único que distingue "te estoy escuchando" de "estoy grabando
    /// silencio", que es el pedido entero. La cuenta no se hace acá: el nivel lo
    /// acumula el propio callback de audio en átomos (ver
    /// [`mithflow_core::audio::Medidor`]) y esto sólo lo lee y lo empaqueta.
    fn publicar_el_nivel(&self) {
        if !superpuesta::hay_que_medir(self.estado.clave(), self.indicador) {
            return;
        }
        let Some(grabadora) = self.grabadora.as_ref() else {
            return;
        };
        eventos::nivel_audio(
            &self.app,
            NivelAudio::nuevo(
                grabadora.tomar_nivel(),
                grabadora.elapsed_secs(),
                self.limite_grabacion_s,
            ),
        );
    }

    fn procesar(&mut self, mensaje: Mensaje) {
        match mensaje {
            Mensaje::Pulso => self.pulso(),
            Mensaje::Pausa(quiero) => self.fijar_pausa(quiero),
            Mensaje::AlternarPausa => self.fijar_pausa(!self.pausado),
            Mensaje::MotorListo(Ok(resumen)) => {
                eprintln!("{resumen}");
                if pasa_a_listo_cuando_el_motor_avisa(&self.estado) {
                    self.cambiar(Estado::Listo);
                }
            }
            Mensaje::MotorListo(Err(fallo)) => self.motor_no_arranca(fallo),
            Mensaje::AtajoRoto(motivo) => {
                self.avisar_error(&motivo);
                self.cambiar(Estado::Error(motivo));
            }
            // El `Box` es sólo para que la variante no infle el tamaño del enum
            // en la cola; acá ya no hay cola, así que se abre.
            Mensaje::Transcripcion { salida, entrada } => {
                self.termino_de_transcribir(*salida, entrada)
            }
            Mensaje::Ajustados(nuevos) => self.aplicar_ajustes(nuevos),
            Mensaje::ModeloDescargado => self.modelo_descargado(),
        }
    }

    /// Se terminó de bajar un modelo. Acá se cierra el lazo que antes obligaba a
    /// reiniciar la aplicación recién instalada.
    ///
    /// La decisión la toma [`tras_una_descarga`], que sólo mira el estado; lo que
    /// se hace con ella es lo único que necesita al director.
    fn modelo_descargado(&mut self) {
        let decidido = tras_una_descarga(&self.estado);
        eprintln!("modelo descargado con el estado en {:?}: {decidido:?}", self.estado);
        match decidido {
            TrasLaDescarga::Lanzar => self.lanzar_el_motor(),
            // Los otros dos no tocan nada: sólo cuentan por qué. Se nombran uno
            // por uno —nada de comodín— para que una variante nueva de
            // `TrasLaDescarga` no caiga acá en silencio.
            TrasLaDescarga::YaEstaCargando | TrasLaDescarga::AlReiniciar => {
                eventos::aviso(&self.app, decidido.aviso(), "info")
            }
        }
    }

    /// Arranca el motor sin reiniciar la aplicación.
    ///
    /// El estado pasa a [`estado_al_lanzar_en_caliente`] **antes** de que el
    /// modelo esté en memoria, y eso es a propósito: cargar los pesos y compilar
    /// los shaders son entre veinte segundos y un minuto la primera vez en la
    /// máquina. Sin publicar el paso intermedio, el usuario que acaba de bajar el
    /// modelo se queda mirando una pantalla que no cambia y no tiene forma de
    /// saber si la app está haciendo algo.
    ///
    /// Publicarlo además es lo que cierra la puerta a un segundo lanzamiento: la
    /// descarga siguiente ya no ve `SinModelo` (ver [`tras_una_descarga`]).
    fn lanzar_el_motor(&mut self) {
        let cfg = ajustes::cargar(&self.app);
        match motor::resolver_y_lanzar(&self.app, &self.al_director, &cfg) {
            Ok(motor) => {
                self.al_motor = motor.al_motor;
                // Con qué archivo arrancó importa después: es lo que evita
                // pedirle que reinicie a quien elige en Ajustes justo el modelo
                // que se está cargando (ver `aviso_al_cambiar_el_modelo`).
                self.modelo_en_el_motor = motor.modelo;
                self.cambiar(estado_al_lanzar_en_caliente());
                eventos::aviso(&self.app, TrasLaDescarga::Lanzar.aviso(), "info");
            }
            // Bajar el modelo no alcanzó: se cuenta como cualquier otro motor que
            // no arranca, con la misma regla de siempre —lo que falta se dice en
            // ámbar, lo que se rompió en rojo— y el canal viejo queda como estaba.
            Err(fallo) => self.motor_no_arranca(fallo),
        }
    }

    /// El motor no va a poder dictar. Las dos ramas se comportan igual en lo
    /// funcional —no se dicta— y se cuentan distinto a propósito.
    ///
    /// `SinModelo` **no pisa un problema real que ya esté publicado**, por la
    /// misma razón que `MotorListo(Ok)` no pisa un error: si el atajo ya se
    /// rompió, bajar el tono a "falta el modelo" escondería la falla.
    fn motor_no_arranca(&mut self, fallo: FalloDelMotor) {
        match fallo {
            FalloDelMotor::SinModelo(_) => {
                eprintln!("{}", fallo.motivo());
                // Sin tono de error y con nivel "info": nadie pidió nada
                // todavía, la app se acaba de abrir y esto es lo que le toca
                // hacer al usuario, no algo que se rompió.
                eventos::aviso(&self.app, fallo.motivo(), "info");
                if !self.estado.es_falla() {
                    self.cambiar(fallo.estado());
                }
            }
            FalloDelMotor::Roto(_) => {
                self.avisar_error(fallo.motivo());
                self.cambiar(fallo.estado());
            }
        }
    }

    /// El atajo. Cada estado tiene una respuesta y **ninguno se queda callado**:
    /// sin realimentación, "no pasó nada" y "no estaba listo" se ven igual.
    fn pulso(&mut self) {
        match &self.estado {
            Estado::Listo => self.empezar_a_grabar(),
            Estado::Grabando => self.dejar_de_grabar(),
            // Los demás no dictan; lo único que cambia es qué se contesta.
            otro => {
                if let Some(motivo) = respuesta_al_atajo(otro) {
                    self.avisar_error(&motivo);
                }
            }
        }
    }

    /// Aplica lo que es del director y le reenvía al motor lo que es del
    /// dictado. El tope nuevo rige desde la grabación siguiente: cambiarlo en
    /// medio de una ya empezada sería cortarle el dictado al usuario mientras
    /// habla.
    ///
    /// El cambio de modelo se decide **acá y no en el comando** por la misma
    /// razón que la descarga: el comando puede ver que la clave cambió, pero no
    /// si hay un motor ni con qué archivo. Ver [`aviso_al_cambiar_el_modelo`].
    fn aplicar_ajustes(&mut self, nuevos: Box<Ajustes>) {
        self.limite_grabacion_s = nuevos.limite_grabacion_s as f32;
        // El indicador SÍ cambia en caliente, al revés que el tope: apagarlo es
        // una queja ("me molesta esta ventanita") y hacerla esperar a la próxima
        // grabación sería no atenderla. Si se apaga en medio de una, se destruye
        // la ventana y la grabación sigue igual.
        if nuevos.indicador != self.indicador {
            self.indicador = nuevos.indicador;
            superpuesta::aplicar_ajuste(&self.app, self.indicador);
        }
        self.indicador_posicion = nuevos.indicador_posicion.clone();
        // Vuelve a evaluar la visibilidad con lo recién guardado: quien acaba de
        // encender el indicador en medio de una grabación tiene que verlo ahora.
        //
        // La esquina nueva, en cambio, rige desde la aparición siguiente, igual
        // que el tope de grabación: mover la ventanita de golpe mientras el
        // usuario está dictando es un salto en la pantalla a cambio de nada, y
        // el caso normal es elegirla con la ventanita escondida.
        self.reflejar_el_indicador();
        if let Some(grabadora) = self.grabadora.as_ref() {
            grabadora.medir_nivel(self.indicador);
        }
        if nuevos.modelo != self.modelo_elegido {
            self.modelo_elegido = nuevos.modelo.clone();
            if let Some(aviso) = aviso_al_cambiar_el_modelo(
                &self.estado,
                self.modelo_en_el_motor.as_deref(),
                &self.modelo_elegido,
            ) {
                eventos::aviso(&self.app, aviso, "info");
            }
        }
        if self.al_motor.send(AlMotor::Ajustes(nuevos)).is_err() {
            eprintln!("el motor no está escuchando: los ajustes del dictado no se aplicaron");
        }
    }

    fn empezar_a_grabar(&mut self) {
        let mut grabadora = match Recorder::new() {
            Ok(g) => g,
            Err(e) => return self.avisar_error(&format!("No pude preparar el micrófono: {e}")),
        };
        grabadora.fijar_tope(self.limite_grabacion_s);
        // Antes de abrir el dispositivo: el medidor tiene que estar encendido
        // cuando llegue el primer chunk, o el primer cuadro de la ventanita
        // saldría en cero. Con el indicador apagado esto deja el callback de
        // audio exactamente como estaba.
        grabadora.medir_nivel(self.indicador);
        // El micrófono ocupado por otra aplicación entra por acá. Se informa y
        // se sigue en `Listo`: no cierra nada.
        if let Err(e) = grabadora.start() {
            return self.avisar_error(&format!("No pude abrir el micrófono: {e}"));
        }
        // El tono suena DESPUÉS de abrir el micrófono, así que entra dentro de
        // los `TONE_GUARD_SECS` que `Recorder::stop` descarta del principio: el
        // modelo no llega a escucharlo por los parlantes.
        self.sonidos.tocar(Tono::Inicio);
        self.grabadora = Some(grabadora);
        self.cambiar(Estado::Grabando);
    }

    fn dejar_de_grabar(&mut self) {
        let Some(mut grabadora) = self.grabadora.take() else {
            // Inalcanzable: sólo se entra en `Grabando` con la grabadora puesta.
            self.cambiar(Estado::Listo);
            return;
        };
        self.sonidos.tocar(Tono::Fin);

        let audio = match grabadora.stop() {
            Ok(a) => a,
            Err(e) => {
                self.avisar_error(&format!("Se perdió el audio: {e}"));
                self.cambiar(Estado::Listo);
                return;
            }
        };

        let segundos = audio.len() as f32 / config::SAMPLE_RATE as f32;
        if segundos < config::MIN_AUDIO_SECS {
            // Un roce del atajo. No es un error y no merece tono: el de fin ya
            // sonó y alcanza para saber que se cortó.
            eventos::aviso(&self.app, "Muy corto, no lo transcribo.", "info");
            self.cambiar(Estado::Listo);
            return;
        }

        // Acá está la transición que importa: `audio` se MUEVE al motor.
        if self.al_motor.send(AlMotor::Audio(audio)).is_err() {
            self.avisar_error("El motor de transcripción no está disponible.");
            self.cambiar(Estado::Listo);
            return;
        }
        self.cambiar(Estado::Transcribiendo);
    }

    /// Corta sola una grabación que llegó al tope. Sin esto, un atajo apretado
    /// sin querer deja el micrófono abierto y el buffer creciendo a 384 KB/s
    /// hasta que alguien se dé cuenta.
    fn vigilar_tope(&mut self) {
        let pasados = self.grabadora.as_ref().map_or(0.0, Recorder::elapsed_secs);
        if pasados >= self.limite_grabacion_s {
            eventos::aviso(
                &self.app,
                "Llegué al máximo de grabación; transcribo lo que hay.",
                "info",
            );
            self.dejar_de_grabar();
        }
    }

    fn termino_de_transcribir(
        &mut self,
        salida: Result<Option<DictationResult>, String>,
        entrada: Option<history::Entry>,
    ) {
        match salida {
            Ok(Some(resultado)) => {
                self.sonidos.tocar(Tono::Pegado);
                if let Some(entrada) = entrada {
                    eventos::dictado_nuevo(&self.app, &entrada);
                }
                // Sólo tiempos: el texto dictado NO va a los logs. Es lo más
                // sensible que maneja esta app y ya queda en el historial, que
                // es un archivo local del usuario y no un log que se comparte
                // al pedir ayuda.
                eprintln!(
                    "dictado: {:.1} s de audio · transcripción {:.2} s",
                    resultado.audio_s, resultado.transcribe_s
                );
            }
            // Silencio, ruido o una alucinación filtrada. Con tono, como en la
            // versión Python: sin él, el usuario no distingue "no te escuché"
            // de "me colgué".
            Ok(None) => self.avisar_error("No se escuchó nada (¿silencio o ruido?)."),
            Err(e) => self.avisar_error(&format!("El dictado falló: {e}")),
        }
        self.cambiar(Estado::Listo);
    }

    /// Pausar suelta la tecla sin cerrar la aplicación. Si se pausa en plena
    /// grabación se corta y se descarta el audio: seguir grabando "en pausa"
    /// sería una contradicción, y transcribir algo que el usuario acaba de
    /// abandonar es peor.
    fn fijar_pausa(&mut self, quiero: bool) {
        if quiero && matches!(self.estado, Estado::Grabando) {
            if let Some(mut grabadora) = self.grabadora.take() {
                let _ = grabadora.stop();
            }
            self.sonidos.tocar(Tono::Fin);
            eventos::aviso(&self.app, "Pausado: descarté la grabación en curso.", "info");
            self.cambiar(Estado::Listo);
        }
        self.pausado = quiero;
        atajo::fijar_pausa(quiero);
        self.publicar();
    }

    fn cambiar(&mut self, nuevo: Estado) {
        if self.estado == nuevo {
            return;
        }
        self.estado = nuevo;
        self.publicar();
    }

    /// Un único lugar donde el estado sale del director: espejo para los
    /// comandos, evento para la interfaz, bandeja y ventanita para cuando no hay
    /// ventana principal.
    ///
    /// El orden no es casual: **la ventanita va última**. Mostrarla despacha al
    /// hilo de la interfaz y espera, y lo que va antes —el espejo, el evento y
    /// la bandeja— no tiene por qué quedar atrás de eso. En la transición que
    /// importa (empezar a grabar) el micrófono ya está abierto y capturando
    /// desde antes de entrar acá, así que nada de esto le come audio al usuario.
    fn publicar(&self) {
        let dto = EstadoDto::nuevo(&self.estado, self.pausado);
        println!(
            "estado: {}{}",
            dto.estado,
            if dto.pausado { " (pausado)" } else { "" }
        );
        self.espejo.escribir(dto.clone());
        eventos::estado_cambiado(&self.app, &dto);
        bandeja::actualizar(&self.app, &dto);
        superpuesta::reflejar(&self.app, &dto, self.indicador, &self.indicador_posicion);
    }

    /// Vuelve a evaluar si la ventanita corresponde, sin publicar nada más. Se
    /// usa al guardar Ajustes: el estado no cambió, pero la respuesta sí puede
    /// haber cambiado.
    fn reflejar_el_indicador(&self) {
        let dto = EstadoDto::nuevo(&self.estado, self.pausado);
        superpuesta::reflejar(&self.app, &dto, self.indicador, &self.indicador_posicion);
    }

    /// Informa un problema recuperable: tono, evento y registro. **Nunca cambia
    /// el estado** — quien llama decide si además hay que moverse.
    fn avisar_error(&self, texto: &str) {
        eprintln!("{texto}");
        self.sonidos.tocar(Tono::Error);
        eventos::aviso(&self.app, texto, "error");
    }
}

/// Qué contestarle al atajo cuando no se puede dictar.
///
/// `None` en los dos estados que SÍ dictan: ésos no se contestan, se atienden
/// moviéndose ([`Director::pulso`]). Es una función suelta y no un método
/// porque no toca nada del director, y así se puede probar sin un `AppHandle`.
fn respuesta_al_atajo(estado: &Estado) -> Option<String> {
    match estado {
        Estado::Listo | Estado::Grabando => None,
        // La misma cifra que dicen el asistente y la documentación: cargar los
        // pesos y compilar los shaders son entre veinte segundos y un minuto la
        // primera vez en cada máquina. "Unos segundos" prometía menos de lo que
        // tarda, y quien apreta la tecla a los diez segundos cree que se colgó.
        Estado::Cargando => Some(
            "Todavía estoy preparando el motor: la primera vez tarda entre veinte segundos y \
             un minuto."
                .into(),
        ),
        Estado::Transcribiendo => Some("Estoy transcribiendo el dictado anterior.".into()),
        // Falta un paso, y se nombra el paso. El motivo guardado dice lo mismo
        // pero mirando hacia atrás ("no hay ningún modelo descargado"); al que
        // acaba de apretar la tecla hay que decirle qué hacer.
        Estado::SinModelo(_) => {
            Some("Todavía no descargaste el modelo. Abrí Ajustes y bajá uno para dictar.".into())
        }
        Estado::Error(motivo) => Some(motivo.clone()),
    }
}

/// ¿Un motor que terminó de cargar puede publicar `Listo` desde este estado?
///
/// **Sólo desde `Cargando`.** Si mientras el motor cargaba algo falló —el atajo,
/// por ejemplo— pisar esa falla con un "Listo" la escondería. Y es también la
/// puerta por la que sale el arranque en caliente: [`Director::lanzar_el_motor`]
/// publica `Cargando` justamente para poder cruzarla, así que las dos mitades
/// tienen que moverse juntas o el motor terminaría de cargar y la interfaz se
/// quedaría en "Cargando…" para siempre.
fn pasa_a_listo_cuando_el_motor_avisa(estado: &Estado) -> bool {
    matches!(estado, Estado::Cargando)
}

/// El estado que [`Director::lanzar_el_motor`] publica apenas el motor arranca
/// en caliente.
///
/// Es una función y no un literal adentro de `lanzar_el_motor` porque es **el
/// eslabón del que cuelga todo lo demás** y los tests tienen que poder
/// aseverarlo en vez de volver a escribirlo a mano: que sea `Cargando` es lo
/// que cierra la puerta al segundo motor de la descarga siguiente
/// ([`tras_una_descarga`]) y, a la vez, lo único que después deja pasar el
/// `Listo` del motor ([`pasa_a_listo_cuando_el_motor_avisa`]). Un test que
/// escriba `Estado::Cargando` de su lado sigue verde aunque acá cambie, que es
/// exactamente cómo se lanzarían dos motores de 1,5 GB sin que nadie se entere.
fn estado_al_lanzar_en_caliente() -> Estado {
    Estado::Cargando
}

/// Qué corresponde hacer cuando una descarga termina bien.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrasLaDescarga {
    /// No hay motor y no lo hubo nunca: se lanza ahora, sin reiniciar. Es el
    /// caso del primer arranque, el que este arreglo vino a cerrar.
    Lanzar,
    /// Ya hay un motor cargándose —el del arranque, o el de la descarga
    /// anterior—. Lanzar otro serían dos modelos de hasta 1,5 GB compilando
    /// shaders al mismo tiempo, y el que sobrevive sería cualquiera de los dos.
    YaEstaCargando,
    /// El motor ya está cargado, o hay una falla que una descarga no arregla.
    /// Reemplazar el modelo en caliente son 1,5 GB y ~2 s de pausa en medio del
    /// trabajo del usuario: el cambio queda para el próximo arranque, que es lo
    /// que esta app viene haciendo desde siempre con el cambio de modelo.
    AlReiniciar,
}

impl TrasLaDescarga {
    /// Lo que se le cuenta al usuario.
    ///
    /// **Ninguno de los tres puede decir que no hay ningún modelo descargado**:
    /// se acaba de bajar uno, y esa contradicción —la app pidiendo lo que el
    /// usuario ya hizo— es el defecto que se está arreglando.
    ///
    /// Y la simetría, que es igual de importante: los dos casos en los que el
    /// modelo recién bajado NO es el que se va a usar ahora **sí** dicen que
    /// hace falta reiniciar. `YaEstaCargando` entra ahí porque el motor está
    /// cargando otro archivo: no se puede estar bajando el que ya se está
    /// cargando, así que callarlo sería la misma mentira al revés.
    fn aviso(self) -> &'static str {
        match self {
            TrasLaDescarga::Lanzar => {
                "Modelo descargado. Estoy cargando el motor; en cuanto diga «Listo» podés dictar."
            }
            TrasLaDescarga::YaEstaCargando => {
                "Modelo descargado. Estoy terminando de cargar el motor; en cuanto diga «Listo» podés dictar. Este modelo se usa cuando reinicies MithFlow."
            }
            TrasLaDescarga::AlReiniciar => {
                "Modelo descargado. El cambio de modelo aplica cuando reinicies MithFlow."
            }
        }
    }
}

/// La regla, en un solo lugar y sin tocar nada: **el motor se lanza si y sólo si
/// no hay ninguno**. Es una función suelta —como [`respuesta_al_atajo`]— para
/// poder probarla sin levantar una aplicación de Tauri, que es justo lo que hace
/// falta acá: la carrera de dos descargas seguidas se decide en este `match`.
fn tras_una_descarga(estado: &Estado) -> TrasLaDescarga {
    match estado {
        // Lo único que impide dictar es que falte el modelo, y ya no falta.
        Estado::SinModelo(_) => TrasLaDescarga::Lanzar,
        Estado::Cargando => TrasLaDescarga::YaEstaCargando,
        // `Error` entra acá y no en `Lanzar` a propósito: puede ser el atajo que
        // no se enganchó o el historial que no se puede escribir, y ninguno de
        // los dos se arregla con un `.gguf` nuevo. Arrancar un motor encima
        // taparía la falla con un "Listo" que no sería cierto.
        Estado::Listo | Estado::Grabando | Estado::Transcribiendo | Estado::Error(_) => {
            TrasLaDescarga::AlReiniciar
        }
    }
}

/// El aviso que corresponde cuando el usuario elige otro modelo en Ajustes, o
/// `None` si no hay nada que contar.
///
/// # Qué se estaba diciendo mal
///
/// El comando avisaba "El modelo cambia la próxima vez que abras MithFlow"
/// mirando **sólo** que la clave guardada fuera distinta de la anterior. En el
/// camino más común del asistente eso es falso: el ajuste arranca en `"auto"`,
/// se baja el `Q4_K_M`, el motor empieza a cargar **ese** archivo, el perfilado
/// lo recomienda y el asistente escribe `modelo: "Q4_K_M"`. La clave cambió,
/// pero el modelo no: el usuario recibía "reiniciá" entre el aviso de que el
/// motor está cargando y la pantalla final que dice que ya puede dictar. Tres
/// mensajes, dos contradictorios, en el primer minuto de uso.
///
/// # Por qué la decisión vive acá
///
/// Porque acá está la información. La pregunta "¿esto se usa ahora o al
/// reiniciar?" es la **misma** que después de una descarga, así que la contesta
/// el mismo [`tras_una_descarga`] y no una regla paralela que pueda divergir. Lo
/// único que se agrega es lo que una descarga no necesita: si el archivo que el
/// motor tiene entre manos ya es el que se acaba de elegir, no cambia nada y no
/// hay nada que avisar.
fn aviso_al_cambiar_el_modelo(
    estado: &Estado,
    en_el_motor: Option<&Path>,
    elegido: &str,
) -> Option<&'static str> {
    if es_el_mismo_modelo(en_el_motor, elegido) {
        return None;
    }
    match tras_una_descarga(estado) {
        // No hay motor: no hay nada que reiniciar. El modelo elegido se va a
        // usar apenas haya uno, que es lo que hace la descarga siguiente.
        TrasLaDescarga::Lanzar => None,
        // Hay un motor —cargándose o cargado— con otro archivo, o una falla que
        // sólo se sale reiniciando. Acá el aviso es verdad.
        TrasLaDescarga::YaEstaCargando | TrasLaDescarga::AlReiniciar => {
            Some("El modelo cambia la próxima vez que abras MithFlow.")
        }
    }
}

/// ¿El motor está cargando (o ya cargó) justamente el modelo que se eligió?
///
/// Se compara por **nombre de archivo** y no por ruta completa, por la misma
/// razón que en `motor::es_el_modelo_de_perfilado`: `MITHFLOW_MODELO` puede
/// apuntar al mismo modelo fuera de `%APPDATA%`, y lo que decide es cuál es, no
/// dónde está guardado.
///
/// `"auto"` contesta `false` a propósito: no nombra un archivo, así que no se
/// puede afirmar que sea el mismo sin volver a resolver la sustitución —que mide
/// memoria y mira el disco— desde el hilo del director. Conservador en la
/// dirección correcta: en el peor caso se avisa de más, nunca se niega lo que el
/// usuario acaba de hacer.
fn es_el_mismo_modelo(en_el_motor: Option<&Path>, elegido: &str) -> bool {
    let (Some(cargado), Some(modelo)) = (en_el_motor, ajustes::modelo_de_clave(elegido)) else {
        return false;
    };
    cargado
        .file_name()
        .is_some_and(|nombre| nombre == modelo.nombre_archivo())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mithflow_core::models::Modelo;

    /// `AlDirector` y `Sonidos` viven en el estado manejado por Tauri, que
    /// exige `Send + Sync + 'static`. Si `mpsc::Sender` dejara de ser `Sync`,
    /// el error saldría acá con nombre y apellido en vez de en `manage`.
    #[test]
    fn los_handles_compartidos_siguen_siendo_send_y_sync() {
        fn exigir<T: Send + Sync + 'static>() {}
        exigir::<AlDirector>();
        exigir::<Sonidos>();
        exigir::<Arc<EstadoCompartido>>();
    }

    /// Enviar a un director que ya no existe no puede paniquear: los clics del
    /// menú de la bandeja pasan por acá.
    #[test]
    fn enviar_sin_director_no_rompe() {
        let (tx, rx) = std::sync::mpsc::channel();
        let al_director = AlDirector::nuevo(tx);
        drop(rx);
        al_director.enviar(Mensaje::AlternarPausa);
    }

    /// Ningún estado que no dicta se queda callado: apretar el atajo siempre
    /// contesta algo. Es la garantía que documenta `pulso`.
    #[test]
    fn el_atajo_contesta_en_todos_los_estados_que_no_dictan() {
        assert_eq!(respuesta_al_atajo(&Estado::Listo), None);
        assert_eq!(respuesta_al_atajo(&Estado::Grabando), None);
        for estado in [
            Estado::Cargando,
            Estado::Transcribiendo,
            Estado::SinModelo("no hay modelo".into()),
            Estado::Error("el atajo no se enganchó".into()),
        ] {
            let respuesta = respuesta_al_atajo(&estado)
                .unwrap_or_else(|| panic!("{estado:?} se quedó sin respuesta"));
            assert!(!respuesta.is_empty(), "{estado:?} contestó vacío");
        }
    }

    /// `SinModelo` dicta tan poco como `Error`, pero lo dice distinto: nombra
    /// el paso que falta en vez del tono de fallo.
    #[test]
    fn sin_modelo_contesta_lo_que_falta_y_no_un_error() {
        let respuesta = respuesta_al_atajo(&Estado::SinModelo(
            "todavía no hay ningún modelo descargado".into(),
        ))
        .expect("sin modelo no se puede dictar, así que hay respuesta");

        assert!(
            respuesta.contains("descargaste") && respuesta.contains("Ajustes"),
            "tiene que decir qué hacer: {respuesta}"
        );
        assert!(
            !respuesta.to_lowercase().contains("error"),
            "el primer arranque no es un error: {respuesta}"
        );
    }

    /// Y un error real contesta su propio motivo, tal cual: nada de suavizarlo.
    #[test]
    fn un_error_real_contesta_su_motivo() {
        let motivo = "No pude enganchar el atajo: otra app tiene la tecla.";
        assert_eq!(
            respuesta_al_atajo(&Estado::Error(motivo.into())).as_deref(),
            Some(motivo)
        );
    }

    /// El defecto que este arreglo cierra: la app se instala sin modelo, el
    /// usuario baja uno **desde la propia app** y hasta acá había que cerrarla y
    /// volver a abrirla para poder dictar. Con el motor ausente, una descarga
    /// que termina bien tiene que lanzarlo.
    #[test]
    fn una_descarga_con_el_motor_ausente_lo_lanza() {
        let sin_modelo = Estado::SinModelo(
            "todavía no hay ningún modelo descargado. Bajá uno desde Ajustes para poder dictar."
                .into(),
        );
        assert_eq!(tras_una_descarga(&sin_modelo), TrasLaDescarga::Lanzar);
    }

    /// Y la otra mitad, que es la que no se puede aflojar: con el motor ya
    /// cargado NO se relanza. Cambiar el modelo en caliente son 1,5 GB y una
    /// pausa en medio del trabajo del usuario; eso sigue siendo cosa del próximo
    /// arranque.
    #[test]
    fn una_descarga_con_el_motor_corriendo_no_lo_relanza() {
        for estado in [Estado::Listo, Estado::Grabando, Estado::Transcribiendo] {
            assert_eq!(
                tras_una_descarga(&estado),
                TrasLaDescarga::AlReiniciar,
                "{estado:?} tiene un motor cargado: relanzarlo sería reemplazarlo en caliente"
            );
        }
    }

    /// La carrera del asistente: baja el modelo de medición y después el
    /// recomendado, uno atrás del otro. La primera descarga lanza el motor y
    /// publica `Cargando`; la segunda ya no puede ver `SinModelo`, así que no
    /// arranca un segundo motor con otros 1,5 GB de pesos.
    ///
    /// El eslabón del medio **no se escribe a mano acá**: sale de
    /// [`estado_al_lanzar_en_caliente`], que es el mismo valor que publica
    /// `lanzar_el_motor`. Copiándolo, este test seguiría verde aunque el
    /// director dejara de publicarlo — y se lanzarían dos motores.
    #[test]
    fn dos_descargas_seguidas_no_lanzan_dos_motores() {
        let sin_modelo = Estado::SinModelo("todavía no hay ningún modelo".into());
        assert_eq!(tras_una_descarga(&sin_modelo), TrasLaDescarga::Lanzar);

        let tras_lanzar = estado_al_lanzar_en_caliente();
        assert_eq!(
            tras_una_descarga(&tras_lanzar),
            TrasLaDescarga::YaEstaCargando,
            "el estado que deja el primer lanzamiento tiene que frenar al segundo"
        );
        assert!(
            pasa_a_listo_cuando_el_motor_avisa(&tras_lanzar),
            "y tiene que ser el mismo desde el que el motor puede publicar «Listo», \
             o la interfaz se queda en «Cargando…» para siempre"
        );
    }

    /// La invariante que sostiene a las dos mitades: **no puede existir un hilo
    /// de motor vivo con el estado en `SinModelo`**.
    ///
    /// Si existiera, la descarga siguiente vería `SinModelo`, lanzaría otro
    /// motor y habría dos cargando el mismo modelo. Se comprueba por los dos
    /// lados: ningún estado que implique un motor vivo lleva a `Lanzar`, y el
    /// único que sí lleva deja de ser `SinModelo` en el mismo acto de lanzarlo.
    ///
    /// `Error` queda afuera de la lista a propósito: puede tener un motor vivo
    /// (el atajo se rompió con el modelo ya cargado) o no tenerlo (el `.gguf`
    /// está cortado), y por eso no lanza nunca — es la rama conservadora.
    #[test]
    fn ningun_motor_vivo_convive_con_el_estado_sin_modelo() {
        for con_motor in [
            Estado::Cargando,
            Estado::Listo,
            Estado::Grabando,
            Estado::Transcribiendo,
        ] {
            assert_ne!(
                con_motor.clave(),
                Estado::SinModelo(String::new()).clave(),
                "{con_motor:?} implica un motor vivo: no puede verse como «falta el modelo»"
            );
            assert_ne!(
                tras_una_descarga(&con_motor),
                TrasLaDescarga::Lanzar,
                "{con_motor:?} ya tiene motor: lanzar otro serían dos modelos en memoria"
            );
        }

        assert_eq!(
            tras_una_descarga(&Estado::SinModelo("no hay modelo".into())),
            TrasLaDescarga::Lanzar,
            "el único estado sin motor es el único que lo lanza"
        );
        assert_ne!(
            estado_al_lanzar_en_caliente().clave(),
            Estado::SinModelo(String::new()).clave(),
            "apenas se lanza el motor, el estado deja de decir que no hay ninguno"
        );
    }

    /// Una falla real no se tapa arrancando un motor encima: el atajo que no se
    /// enganchó o el historial que no se puede escribir siguen ahí después de
    /// bajar un `.gguf`.
    #[test]
    fn una_falla_real_no_se_arregla_con_una_descarga() {
        assert_eq!(
            tras_una_descarga(&Estado::Error("no pude enganchar el atajo".into())),
            TrasLaDescarga::AlReiniciar
        );
    }

    /// El estado transita `SinModelo` → `Cargando` → `Listo`, y las tres partes
    /// tienen que encajar: `Cargando` es distinto de `SinModelo` (si no, la
    /// interfaz y la bandeja no mostrarían el cambio y la pantalla se vería
    /// muerta durante 40 s) y es el único estado desde el que el motor que
    /// termina de cargar puede publicar `Listo`.
    #[test]
    fn el_estado_transita_de_sin_modelo_a_cargando_y_de_ahi_a_listo() {
        let sin_modelo = Estado::SinModelo("todavía no hay ningún modelo".into());
        assert_eq!(tras_una_descarga(&sin_modelo), TrasLaDescarga::Lanzar);

        let cargando = estado_al_lanzar_en_caliente();
        assert_ne!(
            cargando.clave(),
            sin_modelo.clave(),
            "sin un estado intermedio visible, el usuario no ve que está pasando algo"
        );
        assert!(!cargando.es_falla(), "cargar el motor no es una falla");

        assert!(pasa_a_listo_cuando_el_motor_avisa(&cargando));
        assert!(
            !pasa_a_listo_cuando_el_motor_avisa(&Estado::Error("el atajo se rompió".into())),
            "un 'Listo' no puede pisar una falla ya publicada"
        );
        assert!(!pasa_a_listo_cuando_el_motor_avisa(&sin_modelo));
    }

    /// **Ningún** aviso de descarga puede volver a decir que no hay modelo: se
    /// acaba de bajar uno. Ésa era, literalmente, la mentira del defecto.
    #[test]
    fn ningun_aviso_de_descarga_niega_el_modelo_recien_bajado() {
        for caso in [
            TrasLaDescarga::Lanzar,
            TrasLaDescarga::YaEstaCargando,
            TrasLaDescarga::AlReiniciar,
        ] {
            let texto = caso.aviso().to_lowercase();
            assert!(
                texto.contains("modelo descargado"),
                "{caso:?} tiene que reconocer la descarga: {texto}"
            );
            assert!(
                !texto.contains("no hay ningún modelo") && !texto.contains("bajá uno"),
                "{caso:?} le pide al usuario lo que acaba de hacer: {texto}"
            );
        }
    }

    /// Dónde vive un modelo del catálogo, para los tests que comparan lo que
    /// tiene el motor con lo que se elige en Ajustes.
    fn en_disco(modelo: Modelo) -> PathBuf {
        PathBuf::from("C:\\Users\\quien\\AppData\\Roaming\\MithFlow\\models")
            .join(modelo.nombre_archivo())
    }

    /// **La misma regla, del otro lado: ningún aviso puede negar lo que el
    /// usuario acaba de hacer.** Éste es el camino más común del asistente, y
    /// hasta acá quedaba afuera:
    ///
    /// 1. el ajuste arranca en `"auto"`;
    /// 2. se baja el `Q4_K_M` → "Modelo descargado. Estoy cargando el motor…";
    /// 3. el perfilado lo recomienda y el asistente escribe `modelo: "Q4_K_M"`;
    /// 4. **acá** salía "El modelo cambia la próxima vez que abras MithFlow",
    ///    que era falso: el motor está cargando ese mismo archivo;
    /// 5. la pantalla final dice que ya se puede dictar.
    #[test]
    fn elegir_el_modelo_que_el_motor_ya_esta_cargando_no_avisa_nada() {
        let cargando = estado_al_lanzar_en_caliente();
        let en_el_motor = en_disco(Modelo::Q4KM);

        assert_eq!(
            aviso_al_cambiar_el_modelo(&cargando, Some(&en_el_motor), "Q4_K_M"),
            None,
            "el motor está cargando ese mismo archivo: pedir reiniciar es mentir"
        );

        // Y lo mismo con el motor ya cargado: elegir a mano el que se está
        // usando no cambia nada, así que tampoco hay nada que avisar.
        assert_eq!(
            aviso_al_cambiar_el_modelo(&Estado::Listo, Some(&en_el_motor), "Q4_K_M"),
            None
        );
    }

    /// La mitad simétrica, que es la que no se puede aflojar: elegir un modelo
    /// **distinto** del que el motor tiene sí cambia algo, y recién al
    /// reiniciar. Callarlo sería la mentira al revés.
    #[test]
    fn elegir_otro_modelo_sigue_avisando_que_cambia_al_reiniciar() {
        let en_el_motor = en_disco(Modelo::Q4KM);
        for estado in [
            estado_al_lanzar_en_caliente(),
            Estado::Listo,
            Estado::Grabando,
            Estado::Transcribiendo,
        ] {
            let aviso = aviso_al_cambiar_el_modelo(&estado, Some(&en_el_motor), "F16")
                .unwrap_or_else(|| panic!("{estado:?} tiene un motor con otro modelo"));
            assert!(
                aviso.to_lowercase().contains("reinici") || aviso.contains("próxima vez"),
                "{estado:?}: {aviso}"
            );
        }
    }

    /// Sin motor no hay nada que reiniciar. El primer arranque entra por acá:
    /// el asistente escribe el modelo elegido con la app en `SinModelo`, y
    /// mandarlo a reiniciar una aplicación que todavía no cargó nada es la
    /// misma fricción que el arreglo anterior vino a sacar.
    #[test]
    fn elegir_un_modelo_sin_motor_no_manda_a_reiniciar() {
        let sin_modelo = Estado::SinModelo("todavía no hay ningún modelo".into());
        assert_eq!(aviso_al_cambiar_el_modelo(&sin_modelo, None, "F16"), None);
        assert_eq!(aviso_al_cambiar_el_modelo(&sin_modelo, None, "auto"), None);
    }

    /// Una falla real no se calla: con `Error` el cambio de modelo tampoco
    /// aplica solo, y el usuario tiene que saber que hace falta reiniciar.
    #[test]
    fn con_una_falla_publicada_el_cambio_de_modelo_sigue_avisando() {
        let roto = Estado::Error("el atajo no se enganchó".into());
        assert!(aviso_al_cambiar_el_modelo(&roto, None, "F16").is_some());
    }

    /// La comparación es por nombre de archivo: `MITHFLOW_MODELO` puede apuntar
    /// al mismo modelo fuera de `%APPDATA%` y sigue siendo el mismo modelo.
    /// `"auto"` no nombra ningún archivo, así que nunca cuenta como igual.
    #[test]
    fn el_modelo_del_motor_se_reconoce_por_su_nombre_de_archivo() {
        let en_otro_lado = PathBuf::from("D:\\MithFlow\\app-nativa\\models")
            .join(Modelo::Q5KM.nombre_archivo());
        assert!(es_el_mismo_modelo(Some(&en_otro_lado), "Q5_K_M"));
        assert!(es_el_mismo_modelo(Some(&en_otro_lado), "q5_k_m"), "la clave no distingue mayúsculas");
        assert!(!es_el_mismo_modelo(Some(&en_otro_lado), "F16"));
        assert!(!es_el_mismo_modelo(Some(&en_otro_lado), "auto"));
        assert!(!es_el_mismo_modelo(None, "Q5_K_M"), "sin motor no hay con qué comparar");
    }

    /// Sólo el caso que de verdad lo resuelve deja de pedir reiniciar. Mandar a
    /// reiniciar una app recién instalada cuando el motor está arrancando solo
    /// es la fricción que este arreglo vino a sacar; los otros dos bajaron un
    /// modelo que NO es el que se va a cargar ahora, y callar eso sería la
    /// mentira simétrica.
    #[test]
    fn solo_el_lanzamiento_en_caliente_deja_de_pedir_reiniciar() {
        let lanzar = TrasLaDescarga::Lanzar.aviso().to_lowercase();
        assert!(
            !lanzar.contains("reinici"),
            "el motor arranca solo, no hay nada que reiniciar: {lanzar}"
        );

        for caso in [TrasLaDescarga::YaEstaCargando, TrasLaDescarga::AlReiniciar] {
            assert!(
                caso.aviso().to_lowercase().contains("reinici"),
                "{caso:?} bajó un modelo que no es el que se está cargando: {}",
                caso.aviso()
            );
        }
    }

    /// Los dos avisos que dejan al usuario esperando tienen que decirle qué está
    /// pasando y cuándo termina. Un "Modelo descargado." a secas mientras el
    /// motor tarda 40 s se ve igual que una app colgada.
    #[test]
    fn los_avisos_que_hacen_esperar_dicen_hasta_cuando() {
        for caso in [TrasLaDescarga::Lanzar, TrasLaDescarga::YaEstaCargando] {
            let texto = caso.aviso().to_lowercase();
            assert!(
                texto.contains("el motor") && texto.contains("«listo»"),
                "{caso:?} tiene que decir qué está pasando y cuándo se va a poder dictar: {texto}"
            );
        }
    }
}

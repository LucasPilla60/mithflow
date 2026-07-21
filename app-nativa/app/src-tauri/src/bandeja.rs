//! El ícono de la bandeja: la única interfaz que existe cuando la ventana está
//! cerrada, que es como se usa esta aplicación casi siempre.
//!
//! Muestra el estado de tres formas a la vez —color del ícono, texto del primer
//! ítem del menú y tooltip— porque cada una se ve en un momento distinto: el
//! color de reojo, el tooltip al pasar por arriba, el texto al abrir el menú.
//!
//! Los íconos se dibujan en memoria en vez de empaquetar seis PNG. Son círculos
//! de color plano: a 32 píxeles no hay margen para nada más elaborado, y así el
//! juego de colores vive al lado del `match` que lo elige.

use crate::director::{AlDirector, Mensaje};
use crate::estado::EstadoDto;
use crate::{atajo, ventana};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

/// Id del ícono, para recuperarlo con `tray_by_id` desde el hilo del director.
pub const ID: &str = "principal";

const ID_ESTADO: &str = "estado";
const ID_PAUSA: &str = "pausa";
const ID_DASHBOARD: &str = "dashboard";
const ID_AJUSTES: &str = "ajustes";
const ID_SALIR: &str = "salir";

/// Lado del ícono en píxeles. Windows escala desde acá al tamaño real.
const LADO: u32 = 32;

/// Los ítems que cambian de texto. Se guardan en el estado de Tauri para poder
/// tocarlos desde el hilo del director; `MenuItem` es clonable y sus métodos ya
/// se despachan solos al hilo principal.
pub struct Bandeja {
    estado: MenuItem<Wry>,
    pausa: MenuItem<Wry>,
}

/// Construye el ícono con su menú y lo registra en el estado de la aplicación.
pub fn construir(app: &AppHandle) -> tauri::Result<()> {
    let estado = MenuItem::with_id(app, ID_ESTADO, "Cargando…", false, None::<&str>)?;
    let pausa = MenuItem::with_id(app, ID_PAUSA, "Pausar dictado", true, None::<&str>)?;
    let dashboard = MenuItem::with_id(app, ID_DASHBOARD, "Abrir dashboard", true, None::<&str>)?;
    let ajustes = MenuItem::with_id(app, ID_AJUSTES, "Ajustes", true, None::<&str>)?;
    let salir = MenuItem::with_id(app, ID_SALIR, "Salir", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &estado,
            &PredefinedMenuItem::separator(app)?,
            &pausa,
            &dashboard,
            &ajustes,
            &PredefinedMenuItem::separator(app)?,
            &salir,
        ],
    )?;

    TrayIconBuilder::with_id(ID)
        .icon(icono(&EstadoDto::nuevo(&crate::estado::Estado::Cargando, false)))
        .tooltip("MithFlow — cargando…")
        .menu(&menu)
        // El clic izquierdo abre la ventana; el menú va con el derecho, que es
        // lo que espera cualquiera en Windows.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, evento| match evento.id().as_ref() {
            ID_PAUSA => app.state::<AlDirector>().enviar(Mensaje::AlternarPausa),
            ID_DASHBOARD => ventana::mostrar(app, "dashboard"),
            ID_AJUSTES => ventana::mostrar(app, "ajustes"),
            ID_SALIR => app.exit(0),
            // El ítem de estado está deshabilitado, pero el `_` igual hace
            // falta y documenta que no hay nada que hacer con él.
            _ => {}
        })
        .on_tray_icon_event(|tray, evento| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = evento
            {
                ventana::mostrar(tray.app_handle(), "dashboard");
            }
        })
        .build(app)?;

    app.manage(Bandeja { estado, pausa });
    println!("bandeja lista (ícono '{ID}' con menú de 5 ítems)");
    Ok(())
}

/// Refleja el estado. Los errores se registran y se siguen: la bandeja es
/// realimentación, y quedarse sin ella no puede tumbar un dictado.
pub fn actualizar(app: &AppHandle, dto: &EstadoDto) {
    let Some(bandeja) = app.try_state::<Bandeja>() else {
        // Pasa si el director publica antes de que la bandeja exista.
        return;
    };

    let etiqueta = if dto.pausado {
        format!("{} · pausado", dto.etiqueta)
    } else {
        dto.etiqueta.to_string()
    };
    registrar(bandeja.estado.set_text(&etiqueta), "el texto del estado");
    registrar(
        bandeja.pausa.set_text(if dto.pausado {
            "Reanudar dictado"
        } else {
            "Pausar dictado"
        }),
        "el texto de la pausa",
    );

    if let Some(tray) = app.tray_by_id(ID) {
        registrar(tray.set_icon(Some(icono(dto))), "el ícono");
        registrar(tray.set_tooltip(Some(tooltip(dto))), "el tooltip");
    }
}

fn registrar(resultado: tauri::Result<()>, que: &str) {
    if let Err(e) = resultado {
        eprintln!("no pude actualizar {que} de la bandeja: {e}");
    }
}

/// El texto que se ve al pasar el mouse. Incluye la tecla porque es
/// configurable: sin eso, alguien que cambió el atajo no tiene dónde mirarlo.
fn tooltip(dto: &EstadoDto) -> String {
    if let Some(detalle) = &dto.detalle {
        return format!("MithFlow — {}: {detalle}", dto.etiqueta);
    }
    if dto.pausado {
        return "MithFlow — pausado (el atajo está suelto)".to_string();
    }
    match dto.estado {
        "listo" => format!("MithFlow — listo ({} para dictar)", atajo::tecla_actual()),
        "grabando" => format!("MithFlow — grabando ({} para cortar)", atajo::tecla_actual()),
        _ => format!("MithFlow — {}", dto.etiqueta.to_lowercase()),
    }
}

/// Color RGB de cada estado. La pausa gana sobre todo menos el error: si algo
/// se rompió, eso es lo que hay que ver.
fn color(dto: &EstadoDto) -> [u8; 3] {
    match dto.estado {
        "error" => [0xb3, 0x14, 0x12], // rojo oscuro
        _ if dto.pausado => [0x5f, 0x63, 0x68], // gris azulado
        "cargando" => [0x9a, 0xa0, 0xa6],       // gris
        "listo" => [0x34, 0xa8, 0x53],          // verde
        "grabando" => [0xea, 0x43, 0x35],       // rojo
        "transcribiendo" => [0xfb, 0xbc, 0x04], // ámbar
        _ => [0x9a, 0xa0, 0xa6],
    }
}

/// Dibuja un círculo lleno del color del estado, con el borde suavizado.
///
/// El suavizado no es adorno: un círculo de 32 px sin él se ve como un rombo
/// dentado en la barra de tareas.
fn icono(dto: &EstadoDto) -> Image<'static> {
    let [r, g, b] = color(dto);
    let centro = (LADO as f32 - 1.0) / 2.0;
    let radio = centro - 1.0;

    let mut rgba = Vec::with_capacity((LADO * LADO * 4) as usize);
    for y in 0..LADO {
        for x in 0..LADO {
            let dx = x as f32 - centro;
            let dy = y as f32 - centro;
            let distancia = (dx * dx + dy * dy).sqrt();
            // Un píxel de transición entre lleno y vacío.
            let alfa = ((radio - distancia + 0.5).clamp(0.0, 1.0) * 255.0) as u8;
            rgba.extend_from_slice(&[r, g, b, alfa]);
        }
    }
    Image::new_owned(rgba, LADO, LADO)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::estado::Estado;

    fn dto(estado: Estado, pausado: bool) -> EstadoDto {
        EstadoDto::nuevo(&estado, pausado)
    }

    #[test]
    fn cada_estado_tiene_su_color_y_ninguno_se_repite() {
        let colores = [
            color(&dto(Estado::Cargando, false)),
            color(&dto(Estado::Listo, false)),
            color(&dto(Estado::Grabando, false)),
            color(&dto(Estado::Transcribiendo, false)),
            color(&dto(Estado::Error("x".into()), false)),
        ];
        for (i, a) in colores.iter().enumerate() {
            for b in &colores[i + 1..] {
                assert_ne!(a, b, "dos estados con el mismo color son un estado solo");
            }
        }
    }

    #[test]
    fn el_error_se_ve_aunque_este_pausado() {
        assert_eq!(
            color(&dto(Estado::Error("x".into()), true)),
            color(&dto(Estado::Error("x".into()), false))
        );
        assert_ne!(
            color(&dto(Estado::Listo, true)),
            color(&dto(Estado::Listo, false)),
            "pausado tiene que verse distinto de listo"
        );
    }

    #[test]
    fn el_icono_tiene_el_tamano_declarado_y_es_opaco_en_el_centro() {
        let img = icono(&dto(Estado::Listo, false));
        assert_eq!(img.width(), LADO);
        assert_eq!(img.height(), LADO);
        assert_eq!(img.rgba().len(), (LADO * LADO * 4) as usize);

        let centro = ((LADO / 2 * LADO + LADO / 2) * 4) as usize;
        assert_eq!(img.rgba()[centro + 3], 255, "el centro tiene que ser opaco");
        assert_eq!(img.rgba()[3], 0, "la esquina tiene que ser transparente");
    }

    #[test]
    fn el_tooltip_dice_el_motivo_cuando_hay_error() {
        let t = tooltip(&dto(Estado::Error("no hay modelo".into()), false));
        assert!(t.contains("no hay modelo"), "tooltip: {t}");
    }

    #[test]
    fn el_tooltip_nombra_la_tecla_configurada() {
        atajo::fijar_tecla("F11");
        let t = tooltip(&dto(Estado::Listo, false));
        assert!(t.contains("F11"), "tooltip: {t}");
        atajo::fijar_tecla(atajo::TECLA_POR_DEFECTO);
    }
}

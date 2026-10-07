//! Reintentos del cliente de OpenRouter contra un servidor HTTP falso armado
//! con `std::net::TcpListener`: sin dependencias nuevas y sin salir a la red.
//!
//! El incidente real: OpenRouter mandó las cabeceras de un 200, el proveedor
//! se colgó más de 180 s, el timeout se cumplió leyendo el cuerpo y reqwest lo
//! reportó como «error decoding response body». El trabajo se pausó con un
//! mensaje que no decía qué había pasado.

use entropia_agent::cliente_llm::{AjustesRed, ClienteLlm, ClienteLlmOpenRouter, TurnoAgente};
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Respuesta cruda del servidor falso, por conexión.
enum Guion {
    /// Respuesta HTTP completa, escrita de una vez.
    Completa(String),
    /// Cabeceras y un pedazo de cuerpo, y después silencio: el timeout del
    /// cliente se cumple con la respuesta empezada y nunca terminada.
    Truncada {
        cabeceras: String,
        cuerpo: String,
        silencio: Duration,
    },
}

/// Servidor HTTP mínimo: atiende una conexión por intento y sigue el guion.
struct ServidorFalso {
    url: String,
    pedidos: Arc<AtomicUsize>,
}

impl ServidorFalso {
    fn nuevo(guion: Vec<Guion>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let guion = Arc::new(Mutex::new(guion));
        let pedidos = Arc::new(AtomicUsize::new(0));
        let cola = guion.clone();
        let cuenta = pedidos.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let siguiente = cola.lock().unwrap().remove(0);
                cuenta.fetch_add(1, Ordering::SeqCst);
                std::thread::spawn(move || {
                    consumir_pedido(&mut stream);
                    match siguiente {
                        Guion::Completa(cruda) => {
                            let _ = stream.write_all(cruda.as_bytes());
                        }
                        Guion::Truncada {
                            cabeceras,
                            cuerpo,
                            silencio,
                        } => {
                            let _ = stream.write_all(cabeceras.as_bytes());
                            let _ = stream.write_all(cuerpo.as_bytes());
                            let _ = stream.flush();
                            std::thread::sleep(silencio);
                        }
                    }
                    let _ = stream.flush();
                });
            }
        });
        Self {
            url: format!("http://{addr}/api/v1/chat/completions"),
            pedidos,
        }
    }

    fn pedidos(&self) -> usize {
        self.pedidos.load(Ordering::SeqCst)
    }
}

/// Lee el pedido HTTP completo para que el cliente termine de escribirlo.
fn consumir_pedido(stream: &mut std::net::TcpStream) {
    let mut buf = vec![0u8; 16_384];
    let mut leido = 0;
    loop {
        let Ok(n) = stream.read(&mut buf[leido..]) else {
            return;
        };
        if n == 0 {
            return;
        }
        leido += n;
        if let Some(fin) = buf[..leido].windows(4).position(|w| w == b"\r\n\r\n") {
            let texto = String::from_utf8_lossy(&buf[..leido]).to_string();
            let largo: usize = texto
                .split("Content-Length:")
                .nth(1)
                .and_then(|s| s.split("\r\n").next())
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0);
            let mut cuerpo = leido - (fin + 4);
            while cuerpo < largo {
                let Ok(n) = stream.read(&mut buf) else {
                    return;
                };
                if n == 0 {
                    return;
                }
                cuerpo += n;
            }
            return;
        }
        if leido == buf.len() {
            return;
        }
    }
}

/// Respuesta HTTP completa con cuerpo JSON.
fn respuesta(estado: &str, cuerpo: &str) -> String {
    format!(
        "HTTP/1.1 {estado}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{cuerpo}",
        cuerpo.len()
    )
}

/// Un 200 de OpenRouter con un turno de texto.
fn ok() -> String {
    json!({"choices":[{"message":{"content":"listo"}}],"usage":{"cost":0.5}}).to_string()
}

/// Ajustes cortos: timeout de 300 ms y dos reintentos sin espera.
fn ajustes(url: String) -> AjustesRed {
    AjustesRed {
        timeout: Duration::from_millis(300),
        esperas: vec![Duration::ZERO, Duration::ZERO],
        chat_url: url,
    }
}

fn turno(cliente: &ClienteLlmOpenRouter) -> Result<TurnoAgente, String> {
    cliente.turno_agente(&[json!({"role":"user","content":"hola"})], &[])
}

/// El error de un turno que debía fallar. `TurnoAgente` no implementa Debug,
/// así que `unwrap_err` no sirve acá.
fn error_de(r: Result<TurnoAgente, String>) -> String {
    match r {
        Err(e) => e,
        Ok(_) => panic!("se esperaba un error y el turno salió bien"),
    }
}

#[test]
fn reintenta_un_503_y_sigue_con_el_reintento_contado() {
    let servidor = ServidorFalso::nuevo(vec![
        Guion::Completa(respuesta(
            "503 Service Unavailable",
            "{\"error\":\"saturado\"}",
        )),
        Guion::Completa(respuesta("200 OK", &ok())),
    ]);
    let cliente =
        ClienteLlmOpenRouter::con_ajustes("clave", "modelo", ajustes(servidor.url.clone()));
    match turno(&cliente).unwrap() {
        TurnoAgente::Texto(t) => assert_eq!(t, "listo"),
        _ => panic!("no se pidieron herramientas"),
    }
    assert_eq!(cliente.ultimo_reintentos(), 1);
    assert_eq!(servidor.pedidos(), 2);
}

#[test]
fn reintenta_cuando_el_cuerpo_se_corta_en_el_medio() {
    // El incidente: 200 con cabeceras, cuerpo que nunca termina.
    let servidor = ServidorFalso::nuevo(vec![
        Guion::Truncada {
            cabeceras: "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 200\r\nConnection: close\r\n\r\n".into(),
            cuerpo: "{\"choices\":[".into(),
            silencio: Duration::from_secs(2),
        },
        Guion::Completa(respuesta("200 OK", &ok())),
    ]);
    let cliente =
        ClienteLlmOpenRouter::con_ajustes("clave", "modelo", ajustes(servidor.url.clone()));
    match turno(&cliente).unwrap() {
        TurnoAgente::Texto(t) => assert_eq!(t, "listo"),
        _ => panic!("no se pidieron herramientas"),
    }
    assert_eq!(cliente.ultimo_reintentos(), 1);
    assert_eq!(servidor.pedidos(), 2);
}

#[test]
fn un_401_no_se_reintenta() {
    let servidor = ServidorFalso::nuevo(vec![Guion::Completa(respuesta(
        "401 Unauthorized",
        "{\"error\":\"clave inválida\"}",
    ))]);
    let cliente =
        ClienteLlmOpenRouter::con_ajustes("clave", "modelo", ajustes(servidor.url.clone()));
    let error = error_de(turno(&cliente));
    assert!(error.contains("401"), "{error}");
    assert_eq!(cliente.ultimo_reintentos(), 0);
    assert_eq!(servidor.pedidos(), 1);
}

#[test]
fn tres_503_terminan_en_un_mensaje_honesto() {
    let servidor = ServidorFalso::nuevo(vec![
        Guion::Completa(respuesta("503 Service Unavailable", "{\"error\":\"uno\"}")),
        Guion::Completa(respuesta("503 Service Unavailable", "{\"error\":\"dos\"}")),
        Guion::Completa(respuesta("503 Service Unavailable", "{\"error\":\"tres\"}")),
    ]);
    let cliente =
        ClienteLlmOpenRouter::con_ajustes("clave", "modelo", ajustes(servidor.url.clone()));
    let error = error_de(turno(&cliente));
    assert!(error.contains("503"), "{error}");
    assert!(error.contains("tras 3 intentos"), "{error}");
    assert_eq!(cliente.ultimo_reintentos(), 2);
    assert_eq!(servidor.pedidos(), 3);
}

#[test]
fn cuando_el_tiempo_se_agota_el_mensaje_dice_la_verdad() {
    let truncada = || {
        Guion::Truncada {
        cabeceras: "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 200\r\nConnection: close\r\n\r\n".into(),
        cuerpo: "{\"choices\":[".into(),
        silencio: Duration::from_secs(2),
    }
    };
    let servidor = ServidorFalso::nuevo(vec![truncada(), truncada(), truncada()]);
    let cliente =
        ClienteLlmOpenRouter::con_ajustes("clave", "modelo", ajustes(servidor.url.clone()));
    let error = error_de(turno(&cliente));
    assert!(error.contains("no respondió a tiempo (más de"), "{error}");
    assert!(error.contains("tras 3 intentos"), "{error}");
    assert_eq!(cliente.ultimo_reintentos(), 2);
}

#[test]
fn el_retry_after_del_429_pisa_la_espera_programada() {
    // Con esperas en cero, el único modo de esperar de verdad es honrar el
    // `Retry-After` del proveedor.
    let servidor = ServidorFalso::nuevo(vec![
        Guion::Completa(
            "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 1\r\nContent-Type: application/json\r\nContent-Length: 16\r\nConnection: close\r\n\r\n{\"error\":\"cupo\"}"
                .to_string(),
        ),
        Guion::Completa(respuesta("200 OK", &ok())),
    ]);
    let cliente =
        ClienteLlmOpenRouter::con_ajustes("clave", "modelo", ajustes(servidor.url.clone()));
    let inicio = std::time::Instant::now();
    match turno(&cliente).unwrap() {
        TurnoAgente::Texto(t) => assert_eq!(t, "listo"),
        _ => panic!("no se pidieron herramientas"),
    }
    assert!(
        inicio.elapsed() >= Duration::from_secs(1),
        "{:?}",
        inicio.elapsed()
    );
    assert_eq!(cliente.ultimo_reintentos(), 1);
}

#[test]
fn un_cuerpo_vacio_se_reintenta() {
    let servidor = ServidorFalso::nuevo(vec![
        Guion::Completa(respuesta("200 OK", "")),
        Guion::Completa(respuesta("200 OK", &ok())),
    ]);
    let cliente =
        ClienteLlmOpenRouter::con_ajustes("clave", "modelo", ajustes(servidor.url.clone()));
    match turno(&cliente).unwrap() {
        TurnoAgente::Texto(t) => assert_eq!(t, "listo"),
        _ => panic!("no se pidieron herramientas"),
    }
    assert_eq!(cliente.ultimo_reintentos(), 1);
    assert_eq!(servidor.pedidos(), 2);
}

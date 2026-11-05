//! Cliente LLM de OpenRouter para el agente con tool-calling.
//!
//! Replica el patrón del desktop de EntropIA (endpoint, headers y modelo) y
//! añade soporte para herramientas (function calling).

use std::time::Duration;

use serde_json::{json, Value};

/// Cliente de OpenRouter.
pub struct ClienteLlmOpenRouter {
    client: reqwest::blocking::Client,
    api_key: String,
    model: String,
    chat_url: String,
    /// Esperas entre reintentos: `esperas[i]` vale para el intento `i` que
    /// falló. La cantidad de intentos es `esperas.len() + 1`.
    esperas: Vec<Duration>,
    timeout: Duration,
    ultimo_costo: std::cell::Cell<Option<f64>>,
    reintentos: std::cell::Cell<i64>,
    ultimo_fin: std::cell::RefCell<Option<String>>,
}

/// Ajustes de red del cliente: timeout total, esperas entre reintentos y
/// endpoint. El default reproduce la conducta de producción —180 s, tres
/// intentos con 2 s y 6 s de espera—; los tests lo acortan para no dormir.
#[derive(Clone)]
pub struct AjustesRed {
    pub timeout: Duration,
    pub esperas: Vec<Duration>,
    pub chat_url: String,
}

impl Default for AjustesRed {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(180),
            esperas: vec![Duration::from_secs(2), Duration::from_secs(6)],
            chat_url: ClienteLlmOpenRouter::CHAT_URL.to_string(),
        }
    }
}

impl ClienteLlmOpenRouter {
    /// Endpoint de chat de OpenRouter.
    const CHAT_URL: &'static str = "https://openrouter.ai/api/v1/chat/completions";

    /// Tope de salida por turno. Con 4096 el asistente de archivo se cortaba:
    /// el razonamiento del modelo cuenta dentro del tope y un lote de 48 KB
    /// con citas literales no entraba.
    pub const MAX_TOKENS_SALIDA: u32 = 16_384;

    /// Modelo por defecto de la app EntropIA.
    pub const MODELO_DEFAULT: &'static str = "openai/gpt-5.6-luna-pro";

    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self::con_ajustes(api_key, model, AjustesRed::default())
    }

    /// Constructor con los ajustes de red a la vista: timeout, esperas de
    /// reintento y endpoint (los tests apuntan a un servidor falso).
    pub fn con_ajustes(
        api_key: impl Into<String>,
        model: impl Into<String>,
        ajustes: AjustesRed,
    ) -> Self {
        Self {
            client: reqwest::blocking::Client::builder()
                .timeout(ajustes.timeout)
                .build()
                .unwrap_or_default(),
            api_key: api_key.into(),
            model: model.into(),
            chat_url: ajustes.chat_url,
            esperas: ajustes.esperas,
            timeout: ajustes.timeout,
            ultimo_costo: std::cell::Cell::new(None),
            reintentos: std::cell::Cell::new(0),
            ultimo_fin: std::cell::RefCell::new(None),
        }
    }

    /// Un turno del agente con herramientas. Recibe el historial completo y
    /// las definiciones de herramientas; devuelve texto final o llamadas.
    ///
    /// Los fallos transitorios —timeout, corte de conexión, estados 429/5xx,
    /// cuerpo vacío o ilegible— se reintenan hasta agotar las esperas
    /// configuradas; los estados 400/401/402/403/404 se devuelven de
    /// inmediato, porque otro intento daría lo mismo. El mensaje final dice
    /// qué pasó de verdad y cuántos intentos se hicieron.
    pub fn turno_agente(
        &self,
        mensajes: &[Value],
        herramientas: &[Value],
    ) -> Result<TurnoAgente, String> {
        self.ultimo_costo.set(None);
        self.reintentos.set(0);
        self.ultimo_fin.replace(None);
        let mut request = json!({
            "model": self.model,
            "messages": mensajes,
            "temperature": 0.3,
            "max_tokens": Self::MAX_TOKENS_SALIDA,
        });
        if !herramientas.is_empty() {
            request["tools"] = Value::Array(herramientas.to_vec());
            request["tool_choice"] = json!("auto");
        }

        let mut fallo: Option<(String, Option<Duration>)> = None;
        for intento in 0..=self.esperas.len() {
            if intento > 0 {
                self.reintentos.set(self.reintentos.get() + 1);
                // `Retry-After` (solo en 429) pisa la espera programada.
                let espera = fallo
                    .as_ref()
                    .and_then(|(_, propia)| *propia)
                    .unwrap_or(self.esperas[intento - 1]);
                std::thread::sleep(espera);
            }
            match self.intento(&request) {
                ResultadoIntento::Listo(turno) => return Ok(turno),
                ResultadoIntento::Fatal(motivo) => return Err(motivo),
                ResultadoIntento::Transitorio { motivo, espera } => {
                    fallo = Some((motivo, espera));
                }
            }
        }
        let intentos = self.esperas.len() + 1;
        let motivo = fallo
            .map(|(motivo, _)| motivo)
            .unwrap_or_else(|| "Fallo transitorio de OpenRouter".to_string());
        Err(format!("{motivo} tras {intentos} intentos"))
    }

    /// Un intento completo del turno: envío, lectura del cuerpo y parseo.
    fn intento(&self, request: &Value) -> ResultadoIntento {
        let response = match self
            .client
            .post(&self.chat_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("HTTP-Referer", "https://hlab.com.ar/")
            .header("X-Title", "EntropIA")
            .json(request)
            .send()
        {
            Ok(response) => response,
            Err(e) => return Self::fallo_envio(&e, self.timeout),
        };

        let status = response.status();
        if !status.is_success() {
            // `Retry-After` se honra solo en 429, y nunca más de 30 s.
            let espera = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(|s| Duration::from_secs(s.min(30)));
            let cuerpo = response.text().unwrap_or_default();
            let motivo = Self::motivo_limite_clave(status, &cuerpo)
                .unwrap_or_else(|| format!("OpenRouter devolvió el estado {status}: {cuerpo}"));
            return if Self::reintentable(status) {
                let espera = if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    espera
                } else {
                    None
                };
                ResultadoIntento::Transitorio { motivo, espera }
            } else {
                ResultadoIntento::Fatal(motivo)
            };
        }

        // Algunos proveedores anteceden bytes no UTF-8 al JSON; se localiza el
        // primer '{' y se descarta el prefijo.
        let bytes = match response.bytes() {
            Ok(bytes) => bytes,
            Err(e) => return Self::fallo_cuerpo(&e, self.timeout),
        };
        let inicio = bytes.iter().position(|&b| b == b'{').unwrap_or(0);
        let parsed: Value = match serde_json::from_slice(&bytes[inicio..]) {
            Ok(parsed) => parsed,
            Err(e) => {
                // Cuerpo vacío o ilegible: el proveedor contestó cualquier
                // cosa y otra tentativa puede tener más suerte.
                let motivo = if bytes.is_empty() {
                    "OpenRouter devolvió un cuerpo vacío".to_string()
                } else {
                    format!("No se pudo leer la respuesta de OpenRouter: {e}")
                };
                return ResultadoIntento::Transitorio {
                    motivo,
                    espera: None,
                };
            }
        };
        self.ultimo_costo.set(
            parsed["usage"]["cost"]
                .as_f64()
                .filter(|v| v.is_finite() && *v >= 0.0),
        );

        self.ultimo_fin
            .replace(parsed["choices"][0]["finish_reason"].as_str().map(str::to_string));
        let mensaje = &parsed["choices"][0]["message"];
        if let Some(tool_calls) = mensaje["tool_calls"].as_array() {
            let llamadas: Vec<Llamada> = tool_calls
                .iter()
                .filter_map(|tc| {
                    let id = tc["id"].as_str()?.to_string();
                    let nombre = tc["function"]["name"].as_str()?.to_string();
                    let args_str = tc["function"]["arguments"].as_str().unwrap_or("{}");
                    let argumentos: Value =
                        serde_json::from_str(args_str).unwrap_or_else(|_| json!({}));
                    Some(Llamada {
                        id,
                        nombre,
                        argumentos,
                    })
                })
                .collect();
            if !llamadas.is_empty() {
                return ResultadoIntento::Listo(TurnoAgente::Herramientas(llamadas));
            }
        }

        let texto = mensaje["content"].as_str().unwrap_or("").trim().to_string();
        ResultadoIntento::Listo(TurnoAgente::Texto(texto))
    }

    /// Mensaje para el 403 de una clave que agotó su límite de gasto. El
    /// cuerpo crudo trae la URL de gestión de la clave y no le dice al
    /// historiador qué hacer; se conserva solo el detalle de consumo.
    fn motivo_limite_clave(status: reqwest::StatusCode, cuerpo: &str) -> Option<String> {
        if status != reqwest::StatusCode::FORBIDDEN {
            return None;
        }
        let error = serde_json::from_str::<Value>(cuerpo).ok()?["error"].clone();
        let mensaje = error["message"].as_str().unwrap_or("");
        if error["metadata"]["limit_source"] != "openrouter_key_limit"
            && !mensaje.starts_with("Key limit exceeded")
        {
            return None;
        }
        let detalle = mensaje
            .find('$')
            .map(|i| mensaje[i..].trim().trim_end_matches('.').to_string())
            .filter(|d| !d.is_empty())
            .map(|d| format!(" ({d})"))
            .unwrap_or_default();
        Some(format!("La clave de OpenRouter alcanzó su límite de gasto{detalle}. Subí el límite de la clave en OpenRouter y reanudá la investigación."))
    }

    /// Estados que se reintenan: saturación o mantenimiento del proveedor.
    fn reintentable(status: reqwest::StatusCode) -> bool {
        matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504)
    }

    /// Motivo de un fallo de envío. El timeout y el corte se distinguen porque
    /// el historiador tiene derecho a saber cuál de los dos pasó.
    fn fallo_envio(e: &reqwest::Error, timeout: Duration) -> ResultadoIntento {
        let motivo = if e.is_timeout() {
            format!(
                "OpenRouter no respondió a tiempo (más de {} s)",
                timeout.as_secs()
            )
        } else if e.is_connect() {
            format!("No se pudo conectar con OpenRouter: {e}")
        } else {
            format!("Error de conexión con OpenRouter: {e}")
        };
        if e.is_timeout() || e.is_connect() {
            ResultadoIntento::Transitorio {
                motivo,
                espera: None,
            }
        } else {
            ResultadoIntento::Fatal(motivo)
        }
    }

    /// Motivo de un fallo leyendo el cuerpo: la respuesta empezó a llegar y no
    /// terminó. El timeout total puede caer acá mismo, con el cuerpo a medio
    /// leer, y entonces el motivo sigue siendo el tiempo.
    fn fallo_cuerpo(e: &reqwest::Error, timeout: Duration) -> ResultadoIntento {
        let motivo = if e.is_timeout() {
            format!(
                "OpenRouter no respondió a tiempo (más de {} s)",
                timeout.as_secs()
            )
        } else {
            format!("La conexión con OpenRouter se cortó al leer la respuesta: {e}")
        };
        ResultadoIntento::Transitorio {
            motivo,
            espera: None,
        }
    }
}

/// Resultado de un intento de turno: lo que decide el bucle de reintentos.
enum ResultadoIntento {
    Listo(TurnoAgente),
    /// Se puede volver a probar; `espera` pisa la espera programada.
    Transitorio {
        motivo: String,
        espera: Option<Duration>,
    },
    Fatal(String),
}

/// Resultado de un turno del agente: texto final o llamadas a herramientas.
pub enum TurnoAgente {
    Texto(String),
    Herramientas(Vec<Llamada>),
}

/// Una llamada a herramienta devuelta por el modelo.
pub struct Llamada {
    pub id: String,
    pub nombre: String,
    pub argumentos: Value,
}

/// Abstracción del cliente LLM para composiciones de rol (PLAN §5): cada rol
/// (orquestador, worker, verificador) es un prompt de sistema + un set de
/// herramientas + un tope de pasos sobre el mismo cliente. Los tests usan un
/// doble que responde guiones; producción usa `ClienteLlmOpenRouter`.
pub trait ClienteLlm {
    /// Un turno del rol: recibe el historial y las definiciones de
    /// herramientas; devuelve texto final o llamadas a herramientas.
    fn turno_agente(
        &self,
        mensajes: &[Value],
        herramientas: &[Value],
    ) -> Result<TurnoAgente, String>;

    /// Modelo activo (para el snapshot de reproducibilidad del job).
    fn modelo(&self) -> &str;

    /// Costo reportado por el proveedor para el último intento; None no significa cero.
    fn ultimo_costo(&self) -> Option<f64> {
        None
    }

    /// Reintentos de red consumidos por la última llamada; 0 si no hubo.
    fn ultimo_reintentos(&self) -> i64 {
        0
    }

    /// `finish_reason` del último turno según el proveedor; `Some("length")`
    /// dice que la respuesta se cortó en el tope de salida.
    fn ultimo_fin(&self) -> Option<String> {
        None
    }
}

impl ClienteLlm for ClienteLlmOpenRouter {
    fn turno_agente(
        &self,
        mensajes: &[Value],
        herramientas: &[Value],
    ) -> Result<TurnoAgente, String> {
        ClienteLlmOpenRouter::turno_agente(self, mensajes, herramientas)
    }

    fn modelo(&self) -> &str {
        &self.model
    }

    fn ultimo_costo(&self) -> Option<f64> {
        self.ultimo_costo.get()
    }

    fn ultimo_reintentos(&self) -> i64 {
        self.reintentos.get()
    }

    fn ultimo_fin(&self) -> Option<String> {
        self.ultimo_fin.borrow().clone()
    }
}

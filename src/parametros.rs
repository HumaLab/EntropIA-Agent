//! Parámetros configurables del agente.
//!
//! Todo lo que el motor usaba como constante —tope de salida del modelo,
//! temperatura, timeout, esperas de reintento, tamaños de lote del archivo,
//! ventana de la cita, piso de preguntas, profundidades de recuperación y
//! topes de consultas por modalidad— vive acá. El desktop los edita en la
//! solapa «Agente» de Configuración y los manda al crear la investigación.
//!
//! El motor los **valida**, los congela como artefacto `parameters` del job y
//! los usa en cada paso: un `resume` o un job viejo no cambian de conducta a
//! mitad de camino. Un job sin artefacto —los creados antes de existir esta
//! estructura— corre con los defaults, que son exactamente los valores que
//! estaban hardcodeados.
//!
//! Los rangos admitidos están en [`rangos`] y se documentan, con el efecto de
//! subir o bajar cada parámetro, en `docs/parametros-agente.md`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cliente_llm::ClienteLlmOpenRouter;
use crate::investigacion::{PREGUNTAS_MINIMAS, VENTANA_CITA};
use crate::perfiles;
use crate::recuperacion::{AjustesRecuperacion, LEG_K, RERANK_DEPTH, RRF_K, SNIPPET_MAX};

/// Rangos admitidos de cada parámetro. La validación de
/// [`ParametrosAgente::validar`] y la tabla de `docs/parametros-agente.md`
/// hablan de los mismos números: cambiar uno sin el otro es un bug.
pub mod rangos {
    /// Tope de salida por turno, en tokens.
    pub const MAX_TOKENS_SALIDA: (u32, u32) = (1024, 131_072);
    /// Temperatura del pedido al modelo.
    pub const TEMPERATURA: (f64, f64) = (0.0, 2.0);
    /// Timeout total de cada intento, en segundos.
    pub const TIMEOUT_S: (u64, u64) = (1, 3_600);
    /// Cantidad máxima de esperas entre reintentos (los intentos son una más).
    pub const ESPERAS_REINTENTO: usize = 8;
    /// Espera individual entre reintentos, en segundos.
    pub const ESPERA_REINTENTO_S: (u64, u64) = (0, 120);
    /// Cantidad máxima de topes de la escalera de lotes de archivo.
    pub const LOTES_ARCHIVO: usize = 5;
    /// Bytes de un tope de lote de archivo.
    pub const LOTE_ARCHIVO_BYTES: (usize, usize) = (1_000, 1_000_000);
    /// Bytes del fragmento en que se parte una evidencia larga.
    pub const FRAGMENTO_ARCHIVO_BYTES: (usize, usize) = (200, 1_000_000);
    /// Caracteres del fragmento que el informe reproduce por cita.
    pub const VENTANA_CITA: (usize, usize) = (100, 100_000);
    /// Preguntas mínimas de la ronda de clarificación.
    pub const PREGUNTAS_MINIMAS: (usize, usize) = (1, 8);
    /// Candidatos que entran al rerank (y techo del `retrieval_limit`).
    pub const RERANK_DEPTH: (usize, usize) = (1, 200);
    /// Candidatos por pierna de recuperación.
    pub const LEG_K: (usize, usize) = (1, 1_000);
    /// Constante de suavizado de RRF.
    pub const RRF_K: (usize, usize) = (1, 10_000);
    /// Caracteres por fragmento en el rerank y en la búsqueda léxica.
    pub const SNIPPET_MAX: (usize, usize) = (100, 100_000);
    /// Consultas del plan por modalidad de informe.
    pub const MAX_CONSULTAS: (usize, usize) = (1, 200);
}

/// Nombres de los campos, en el orden del contrato JSON. Es la lista que
/// viaja en el error de un campo desconocido.
pub const CAMPOS: [&str; 13] = [
    "max_tokens_salida",
    "temperatura",
    "timeout_s",
    "esperas_reintento_s",
    "lotes_archivo_bytes",
    "fragmento_archivo_bytes",
    "ventana_cita",
    "preguntas_minimas",
    "rerank_depth",
    "leg_k",
    "rrf_k",
    "snippet_max",
    "max_consultas",
];

/// Parámetros del agente, congelados en el job al crearlo.
///
/// `#[serde(default)]` permite mandar un objeto parcial —lo que no viene queda
/// en su default— y `deny_unknown_fields` fija el contrato: un campo mal
/// escrito no se ignora en silencio.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ParametrosAgente {
    /// Tope de salida por turno del modelo, en tokens.
    pub max_tokens_salida: u32,
    /// Temperatura del pedido al modelo.
    pub temperatura: f64,
    /// Timeout total de cada intento, en segundos.
    pub timeout_s: u64,
    /// Esperas entre reintentos de red, en segundos. `esperas[i]` vale para el
    /// intento `i` que falló: la cantidad de intentos es `len + 1`.
    pub esperas_reintento_s: Vec<u64>,
    /// Topes de insumo del lote de archivo, de mayor a menor: cada reintento
    /// del mismo lote usa el siguiente.
    pub lotes_archivo_bytes: Vec<usize>,
    /// Tamaño con que se parte una evidencia que no entra en un fragmento.
    pub fragmento_archivo_bytes: usize,
    /// Caracteres del fragmento que el informe reproduce por cita.
    pub ventana_cita: usize,
    /// Preguntas mínimas de la ronda de clarificación.
    pub preguntas_minimas: usize,
    /// Candidatos que entran al rerank y techo del `retrieval_limit` del plan.
    pub rerank_depth: usize,
    /// Candidatos por pierna de recuperación (vectorial y léxica).
    pub leg_k: usize,
    /// Constante de suavizado de la fusión RRF.
    pub rrf_k: usize,
    /// Caracteres por fragmento en el rerank y en la salida de la búsqueda.
    pub snippet_max: usize,
    /// Tope de consultas del plan, por identificador de modalidad.
    pub max_consultas: BTreeMap<String, usize>,
}

impl Default for ParametrosAgente {
    fn default() -> Self {
        Self {
            max_tokens_salida: ClienteLlmOpenRouter::MAX_TOKENS_SALIDA,
            temperatura: 0.3,
            timeout_s: 180,
            esperas_reintento_s: vec![2, 6],
            lotes_archivo_bytes: vec![48_000, 24_000, 12_000],
            fragmento_archivo_bytes: 8_000,
            ventana_cita: VENTANA_CITA,
            preguntas_minimas: PREGUNTAS_MINIMAS,
            rerank_depth: RERANK_DEPTH,
            leg_k: LEG_K,
            rrf_k: RRF_K,
            snippet_max: SNIPPET_MAX,
            max_consultas: perfiles::cuotas()
                .into_iter()
                .map(|(id, tope)| (id.to_string(), tope))
                .collect(),
        }
    }
}

impl ParametrosAgente {
    /// Lee `parameters` de un pedido.
    ///
    /// Ausente o nulo vale los defaults; un objeto parcial se completa con
    /// ellos. Un campo desconocido, un tipo equivocado o un valor fuera de
    /// rango se rechazan con un mensaje que nombra el campo.
    pub fn desde_valor(valor: &Value) -> Result<Self, String> {
        if valor.is_null() {
            return Ok(Self::default());
        }
        if !valor.is_object() {
            return Err(
                "parameters debe ser un objeto JSON con los parámetros del agente".to_string(),
            );
        }
        let parametros: Self =
            serde_json::from_value(valor.clone()).map_err(|e| mensaje_serde(&e))?;
        parametros.validar()?;
        Ok(parametros)
    }

    /// Valida cada parámetro contra su rango y las reglas que cruzan campos.
    ///
    /// El mensaje nombra el campo (con índice cuando es una lista), el rango
    /// admitido y el valor recibido: quien lo lee no tiene que adivinar qué
    /// corregir en la solapa de configuración.
    pub fn validar(&self) -> Result<(), String> {
        en_rango(
            "max_tokens_salida",
            self.max_tokens_salida,
            rangos::MAX_TOKENS_SALIDA,
        )?;
        let (min, max) = rangos::TEMPERATURA;
        if !(min..=max).contains(&self.temperatura) {
            return Err(format!(
                "temperatura debe estar entre {min:.1} y {max:.1} (recibido: {})",
                self.temperatura
            ));
        }
        en_rango("timeout_s", self.timeout_s, rangos::TIMEOUT_S)?;
        if self.esperas_reintento_s.len() > rangos::ESPERAS_REINTENTO {
            return Err(format!(
                "esperas_reintento_s debe tener entre 0 y {} valores (recibido: {})",
                rangos::ESPERAS_REINTENTO,
                self.esperas_reintento_s.len()
            ));
        }
        for (i, espera) in self.esperas_reintento_s.iter().enumerate() {
            en_rango_de_lista(
                "esperas_reintento_s",
                i,
                *espera,
                rangos::ESPERA_REINTENTO_S,
            )?;
        }
        if self.lotes_archivo_bytes.is_empty()
            || self.lotes_archivo_bytes.len() > rangos::LOTES_ARCHIVO
        {
            return Err(format!(
                "lotes_archivo_bytes debe tener entre 1 y {} valores (recibido: {})",
                rangos::LOTES_ARCHIVO,
                self.lotes_archivo_bytes.len()
            ));
        }
        for (i, lote) in self.lotes_archivo_bytes.iter().enumerate() {
            en_rango_de_lista("lotes_archivo_bytes", i, *lote, rangos::LOTE_ARCHIVO_BYTES)?;
        }
        // Una escalera que no decrece reintenta el mismo lote: gasta la llamada
        // y deja el archivo donde estaba.
        for (i, par) in self.lotes_archivo_bytes.windows(2).enumerate() {
            if par[1] >= par[0] {
                return Err(format!(
                    "lotes_archivo_bytes debe ser estrictamente decreciente: lotes_archivo_bytes[{}] ({}) no es menor que lotes_archivo_bytes[{}] ({})",
                    i + 1,
                    par[1],
                    i,
                    par[0]
                ));
            }
        }
        en_rango(
            "fragmento_archivo_bytes",
            self.fragmento_archivo_bytes,
            rangos::FRAGMENTO_ARCHIVO_BYTES,
        )?;
        // Un fragmento más grande que el primer lote no tiene lote donde
        // entrar: la evidencia se partiría sin poder procesarse nunca.
        let primero = self.lotes_archivo_bytes[0];
        if self.fragmento_archivo_bytes > primero {
            return Err(format!(
                "fragmento_archivo_bytes ({}) no puede superar el primer lote de archivo ({primero})",
                self.fragmento_archivo_bytes
            ));
        }
        en_rango("ventana_cita", self.ventana_cita, rangos::VENTANA_CITA)?;
        en_rango(
            "preguntas_minimas",
            self.preguntas_minimas,
            rangos::PREGUNTAS_MINIMAS,
        )?;
        en_rango("rerank_depth", self.rerank_depth, rangos::RERANK_DEPTH)?;
        en_rango("leg_k", self.leg_k, rangos::LEG_K)?;
        en_rango("rrf_k", self.rrf_k, rangos::RRF_K)?;
        en_rango("snippet_max", self.snippet_max, rangos::SNIPPET_MAX)?;
        for (id, tope) in &self.max_consultas {
            if perfiles::resolver(id).is_none() {
                return Err(format!(
                    "max_consultas nombra un perfil desconocido: «{id}»; disponibles: {}",
                    perfiles::catalogo()
                        .iter()
                        .map(|(id, _)| *id)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            let (min, max) = rangos::MAX_CONSULTAS;
            if *tope < min || *tope > max {
                return Err(format!(
                    "max_consultas[«{id}»] debe estar entre {min} y {max} (recibido: {tope})"
                ));
            }
        }
        Ok(())
    }

    /// Las constantes de recuperación que este job congela. Es la vista que
    /// consume `Recuperador` sin arrastrar los parámetros del cliente.
    pub fn recuperacion(&self) -> AjustesRecuperacion {
        AjustesRecuperacion {
            rerank_depth: self.rerank_depth,
            leg_k: self.leg_k,
            rrf_k: self.rrf_k,
            snippet_max: self.snippet_max,
        }
    }
}

/// Error de un parámetro escalar fuera de su rango.
fn en_rango<T>(campo: &str, valor: T, (min, max): (T, T)) -> Result<(), String>
where
    T: PartialOrd + std::fmt::Display,
{
    if valor < min || valor > max {
        Err(format!(
            "{campo} debe estar entre {min} y {max} (recibido: {valor})"
        ))
    } else {
        Ok(())
    }
}

/// Igual que [`en_rango`], para un valor dentro de una lista.
fn en_rango_de_lista<T>(
    campo: &str,
    indice: usize,
    valor: T,
    (min, max): (T, T),
) -> Result<(), String>
where
    T: PartialOrd + std::fmt::Display,
{
    if valor < min || valor > max {
        Err(format!(
            "{campo}[{indice}] debe estar entre {min} y {max} (recibido: {valor})"
        ))
    } else {
        Ok(())
    }
}

/// Traduce el error de serde al contrato del motor. Un campo desconocido es el
/// caso que el historiador puede corregir solo, así que se nombra el campo y
/// se listan los válidos; el resto se conserva con el prefijo en castellano.
fn mensaje_serde(e: &serde_json::Error) -> String {
    let texto = e.to_string();
    if let Some(resto) = texto.strip_prefix("unknown field `") {
        if let Some(fin) = resto.find('`') {
            return format!(
                "Parámetros del agente: campo desconocido «{}». Campos válidos: {}",
                &resto[..fin],
                CAMPOS.join(", ")
            );
        }
    }
    format!("Parámetros del agente inválidos: {texto}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_tipo_equivocado_se_rechaza_con_un_mensaje_del_contrato() {
        let error = ParametrosAgente::desde_valor(&serde_json::json!({
            "lotes_archivo_bytes": 48_000
        }))
        .unwrap_err();
        assert!(
            error.starts_with("Parámetros del agente inválidos: ")
                && error.contains("expected a sequence"),
            "{error}"
        );
    }

    #[test]
    fn un_parameters_que_no_es_objeto_se_rechaza() {
        assert_eq!(
            ParametrosAgente::desde_valor(&serde_json::json!("todo por defecto")).unwrap_err(),
            "parameters debe ser un objeto JSON con los parámetros del agente"
        );
    }

    #[test]
    fn una_cuota_de_perfil_fuera_de_rango_se_rechaza() {
        assert_eq!(
            ParametrosAgente::desde_valor(&serde_json::json!({
                "max_consultas": {"general": 0}
            }))
            .unwrap_err(),
            "max_consultas[«general»] debe estar entre 1 y 200 (recibido: 0)"
        );
    }
}

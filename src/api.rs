//! Superficie de integración del motor de investigación (seam IPC).
//!
//! Toda operación devuelve el **snapshot estructurado** del job —`job`,
//! `events`, `artifacts`, `gates`, `sources`— y no texto de presentación. El
//! frontend no tiene que reinterpretar cadenas formateadas para saber qué pasó,
//! ni reimplementar la máquina de estados en TypeScript: la única autoridad
//! sobre las transiciones es `investigacion::procesar`.
//!
//! Los errores del backend viajan como `Err(String)` y no se disfrazan de
//! éxito: un paso que falla deja el job en su estado real y lo dice.

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::cliente_llm::ClienteLlm;
use crate::estado::EstadoDb;
use crate::investigacion;
use crate::recuperacion::Recuperador;
use crate::repositorio::RepositorioSqlite;

/// API de integración sobre el workflow durable de investigación.
pub struct ApiAgente<'a> {
    pub llm: &'a dyn ClienteLlm,
    pub repo: &'a RepositorioSqlite,
    pub db: &'a EstadoDb,
    /// Recuperación híbrida (embeddings + léxica + rerank). En `None` el motor
    /// busca solo por léxico y lo declara en el informe: es un modo degradado
    /// legítimo —sin claves de API o sin red— nunca silencioso.
    pub recuperador: Option<&'a Recuperador>,
    pub dir_artefactos: PathBuf,
}

impl ApiAgente<'_> {
    fn op(&self, request: Value) -> Result<Value, String> {
        investigacion::procesar(
            self.db,
            self.repo,
            self.llm,
            self.recuperador,
            &self.dir_artefactos,
            request,
        )
    }

    /// `research_list`: investigaciones persistidas, colecciones disponibles y
    /// modalidades de informe. Es lo que necesita la vista raíz.
    pub fn research_list(&self) -> Result<Value, String> {
        self.op(json!({"op": "list"}))
    }

    /// `research_create`: crea el job con su recorte y su presupuesto
    /// congelados. `pedido` lleva `question`, `project`, `collection_ids`,
    /// `max_llm_calls` y, opcionalmente, `max_cost`, `context` y `modalidad`.
    pub fn research_create(&self, pedido: Value) -> Result<Value, String> {
        let mut request = pedido;
        request["op"] = json!("create");
        self.op(request)
    }

    /// `research_get`: snapshot del job. La evidencia viene omitida por volumen;
    /// para leer una fuente concreta está `research_source`.
    pub fn research_get(&self, job_id: &str) -> Result<Value, String> {
        self.op(json!({"op": "get", "job_id": job_id}))
    }

    /// `research_step`: ejecuta la siguiente etapa autorizada. Las etapas de
    /// volumen variable avanzan por lotes con checkpoint: un paso puede no
    /// cambiar de fase y eso no es un error.
    pub fn research_step(&self, job_id: &str) -> Result<Value, String> {
        self.op(json!({"op": "advance", "job_id": job_id}))
    }

    /// `research_answer`: responde la ronda de clarificación. `answers` es una
    /// lista de `{id, text}`.
    pub fn research_answer(&self, job_id: &str, answers: Value) -> Result<Value, String> {
        self.op(json!({"op": "answer", "job_id": job_id, "answers": answers}))
    }

    /// `research_decision`: resuelve un gate humano pendiente.
    pub fn research_decision(
        &self,
        job_id: &str,
        gate_id: &str,
        aprobar: bool,
    ) -> Result<Value, String> {
        self.op(json!({"op":"decision","job_id":job_id,"gate_id":gate_id,"approve":aprobar}))
    }

    /// `research_revise`: reemplaza el diseño o el plan. Invalida lo derivado y
    /// lo declara; la evidencia y los juicios son registros inmutables.
    pub fn research_revise(
        &self,
        job_id: &str,
        artifact_id: &str,
        contenido: Value,
    ) -> Result<Value, String> {
        self.op(
            json!({"op":"revise","job_id":job_id,"artifact_id":artifact_id,"content":contenido}),
        )
    }

    /// `research_budget`: ajusta el presupuesto sobre el trabajo ya hecho. No
    /// puede quedar por debajo de lo consumido.
    pub fn research_budget(
        &self,
        job_id: &str,
        max_llm_calls: i64,
        max_cost: Option<f64>,
    ) -> Result<Value, String> {
        self.op(
            json!({"op":"update_budget","job_id":job_id,"max_llm_calls":max_llm_calls,"max_cost":max_cost}),
        )
    }

    /// `research_pause`: la pausa se hace efectiva en el límite de la etapa.
    pub fn research_pause(&self, job_id: &str) -> Result<Value, String> {
        self.op(json!({"op": "pause", "job_id": job_id}))
    }

    pub fn research_resume(&self, job_id: &str) -> Result<Value, String> {
        self.op(json!({"op": "resume", "job_id": job_id}))
    }

    pub fn research_cancelar(&self, job_id: &str) -> Result<Value, String> {
        self.op(json!({"op": "cancel", "job_id": job_id}))
    }

    /// `research_continuar_cobertura`: acepta una advertencia de cobertura
    /// pendiente y sigue con las limitaciones declaradas.
    pub fn research_continuar_cobertura(&self, job_id: &str) -> Result<Value, String> {
        self.op(json!({"op": "continue_coverage", "job_id": job_id}))
    }

    /// `research_source`: ruta del asset original y página para abrir el escaneo
    /// real. Solo resuelve items que pertenecen a esta investigación: una fuente
    /// ajena al recorte no se abre desde acá.
    pub fn research_source(&self, job_id: &str, item_id: &str) -> Result<Value, String> {
        self.op(json!({"op": "source", "job_id": job_id, "item_id": item_id}))
    }

    /// Jobs que pueden retomarse tras cerrar y reabrir la app.
    pub fn hay_jobs_reanudables(&self) -> Result<Vec<String>, String> {
        let listado = self.research_list()?;
        Ok(listado["jobs"]
            .as_array()
            .map(|jobs| {
                jobs.iter()
                    .filter(|j| {
                        matches!(
                            j["status"].as_str(),
                            Some("running") | Some("paused") | Some("awaiting_human")
                        )
                    })
                    .filter_map(|j| j["id"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::estado::EstadoDb;
    use crate::llm_fake::LlmWorkflow;
    use crate::repositorio::RepositorioSqlite;

    fn base() -> (EstadoDb, RepositorioSqlite, PathBuf) {
        let repo =
            RepositorioSqlite::abrir(crate::tests_comunes::corpus_sintetico().to_str().unwrap())
                .unwrap();
        let db = EstadoDb::abrir_en_memoria().unwrap();
        // Directorio único por test: corren en paralelo y comparten el pid.
        let dir = std::env::temp_dir().join(format!(
            "entropia-api-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        (db, repo, dir)
    }

    fn api<'a>(
        llm: &'a LlmWorkflow,
        db: &'a EstadoDb,
        repo: &'a RepositorioSqlite,
        dir: &std::path::Path,
    ) -> ApiAgente<'a> {
        ApiAgente {
            llm,
            repo,
            db,
            recuperador: None,
            dir_artefactos: dir.to_path_buf(),
        }
    }

    fn pedido() -> Value {
        json!({
            "question": "¿Hubo conflictividad en el SOIP?",
            "project": "soip-conflictividad",
            "collection_ids": ["c-conflicto"],
            "max_llm_calls": 40,
            "max_cost": 5.0
        })
    }

    /// Avanza respondiendo la ronda cuando el job frena.
    fn hasta_el_final(api: &ApiAgente, id: &str) -> Value {
        let mut out = api.research_get(id).unwrap();
        for _ in 0..60 {
            match out["job"]["status"].as_str() {
                Some("done") => return out,
                Some("awaiting_human") => {
                    let preguntas = out["artifacts"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .rfind(|a| a["kind"] == "clarification_round" && a["obsolete"] == false)
                        .expect("la ronda tiene que existir")["content"]["questions"]
                        .as_array()
                        .unwrap()
                        .clone();
                    let respuestas: Vec<Value> = preguntas
                        .iter()
                        .map(|q| json!({"id": q["id"], "text": "1965-1966, conflicto gremial"}))
                        .collect();
                    out = api.research_answer(id, json!(respuestas)).unwrap();
                }
                _ => out = api.research_step(id).unwrap(),
            }
        }
        panic!("la investigación no cerró: {}", out["job"]["status"]);
    }

    #[test]
    fn el_ciclo_completo_pasa_por_la_ronda_y_cierra_con_informe() {
        let (db, repo, dir) = base();
        let llm = LlmWorkflow;
        let api = api(&llm, &db, &repo, &dir);

        let creado = api.research_create(pedido()).unwrap();
        let id = creado["job"]["id"].as_str().unwrap().to_owned();
        assert_eq!(creado["job"]["status"], "running");
        assert_eq!(creado["job"]["phase"], "coverage");

        let cerrado = hasta_el_final(&api, &id);
        assert_eq!(cerrado["job"]["status"], "done");
        assert_eq!(cerrado["job"]["close_reason"], "completed");
        assert!(dir.join(&id).join("report.json").exists());
        assert!(dir.join(&id).join("report.md").exists());

        // El listado devuelve el job, las colecciones y las modalidades.
        let listado = api.research_list().unwrap();
        assert!(listado["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|j| j["id"] == id.as_str()));
        assert!(!listado["collections"].as_array().unwrap().is_empty());
        assert!(listado["modalidades"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["id"] == "cronologia"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn los_eventos_y_artefactos_llegan_estructurados_no_como_texto() {
        let (db, repo, dir) = base();
        let llm = LlmWorkflow;
        let api = api(&llm, &db, &repo, &dir);
        let id = api.research_create(pedido()).unwrap()["job"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let out = hasta_el_final(&api, &id);

        // Un evento es un objeto con su tipo y su payload: el frontend no tiene
        // que parsear prosa para saber qué pasó.
        let eventos = out["events"].as_array().unwrap();
        assert!(!eventos.is_empty());
        for e in eventos {
            assert!(e["kind"].is_string(), "{e}");
            assert!(e["timestamp"].is_i64(), "{e}");
        }
        assert!(eventos
            .iter()
            .any(|e| e["kind"] == "clarification_requested"));
        assert!(eventos.iter().any(|e| e["kind"] == "query"));

        // Los artefactos traen su contenido, versión y si quedaron obsoletos.
        let artefactos = out["artifacts"].as_array().unwrap();
        for a in artefactos {
            assert!(a["kind"].is_string(), "{a}");
            assert!(a["version"].is_i64(), "{a}");
            assert!(a["obsolete"].is_boolean(), "{a}");
        }
        assert!(artefactos.iter().any(|a| a["kind"] == "report"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pausa_reanudacion_y_presupuesto_sobre_el_trabajo_existente() {
        let (db, repo, dir) = base();
        let llm = LlmWorkflow;
        let api = api(&llm, &db, &repo, &dir);
        let id = api.research_create(pedido()).unwrap()["job"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        api.research_step(&id).unwrap();

        let pausado = api.research_pause(&id).unwrap();
        assert_eq!(pausado["job"]["status"], "paused");
        // Pausado no se avanza.
        assert!(api.research_step(&id).is_err());

        // El presupuesto no puede quedar por debajo de lo consumido.
        assert!(api.research_budget(&id, 0, None).is_err());
        let ajustado = api.research_budget(&id, 80, Some(9.0)).unwrap();
        assert_eq!(ajustado["job"]["max_llm_calls"], 80);

        let reanudado = api.research_resume(&id).unwrap();
        assert_eq!(reanudado["job"]["status"], "running");
        assert_eq!(api.hay_jobs_reanudables().unwrap(), vec![id.clone()]);

        // Cancelar lo saca de la lista de reanudables.
        api.research_cancelar(&id).unwrap();
        assert!(api.hay_jobs_reanudables().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn la_fuente_se_abre_solo_si_pertenece_a_la_investigacion() {
        let (db, repo, dir) = base();
        let llm = LlmWorkflow;
        let api = api(&llm, &db, &repo, &dir);
        let id = api.research_create(pedido()).unwrap()["job"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        hasta_el_final(&api, &id);

        let fuente = api.research_source(&id, "item-1").unwrap();
        let rutas = fuente["sources"].as_array().unwrap();
        assert_eq!(rutas[0]["path"], "escaneos/65-03-17-a.pdf");
        assert_eq!(rutas[0]["page"], 3);

        // Un item que no entró a esta investigación no se abre desde acá.
        assert!(api.research_source(&id, "item-stress-1").is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn un_error_del_backend_no_se_disfraza_de_exito() {
        let (db, repo, dir) = base();
        let llm = LlmWorkflow;
        let api = api(&llm, &db, &repo, &dir);
        // Recorte sin material procesado: el job no arranca.
        let vacio = api.research_create(json!({
            "question": "¿Hubo conflictividad?",
            "project": "p",
            "collection_ids": ["c-volantes"],
            "max_llm_calls": 20
        }));
        assert!(vacio.is_err(), "{vacio:?}");
        assert!(vacio.unwrap_err().contains("material procesado"));

        // Y un job inexistente tampoco devuelve un snapshot vacío.
        assert!(api.research_get("job-inexistente").is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}

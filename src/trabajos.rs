//! Motor de trabajos largos (PLAN §6.7, Fase 1).
//!
//! Ciclo de vida del job: `planned → running → paused → awaiting_human →
//! done / failed`, con `close_reason` para cierres con motivo. Checkpoints por
//! stage, `resume` crash-safe, reutilización de stages por snapshots +
//! timestamps (§6.2), budgets `max_cost`/`max_llm_calls` y ledger de eventos
//! append-only.

use rusqlite::{params, OptionalExtension};

use crate::estado::{ahora, nuevo_id, EstadoDb};
use crate::grafo;

/// Estados del ciclo de vida de un job (PLAN §6.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstadoJob {
    Planned,
    Running,
    Paused,
    AwaitingHuman,
    Done,
    Failed,
}

impl EstadoJob {
    pub fn as_str(&self) -> &'static str {
        match self {
            EstadoJob::Planned => "planned",
            EstadoJob::Running => "running",
            EstadoJob::Paused => "paused",
            EstadoJob::AwaitingHuman => "awaiting_human",
            EstadoJob::Done => "done",
            EstadoJob::Failed => "failed",
        }
    }

    pub fn desde_str(s: &str) -> Option<Self> {
        match s {
            "planned" => Some(Self::Planned),
            "running" => Some(Self::Running),
            "paused" => Some(Self::Paused),
            "awaiting_human" => Some(Self::AwaitingHuman),
            "done" => Some(Self::Done),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// Motivo de cierre de un job (no es un estado propio: es el porqué del cierre).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotivoCierre {
    Completed,
    Cancelled,
    BudgetExhausted,
    Blocked,
}

impl MotivoCierre {
    pub fn as_str(&self) -> &'static str {
        match self {
            MotivoCierre::Completed => "completed",
            MotivoCierre::Cancelled => "cancelled",
            MotivoCierre::BudgetExhausted => "budget_exhausted",
            MotivoCierre::Blocked => "blocked",
        }
    }
}

/// Un job de investigación persistido.
#[derive(Debug, Clone)]
pub struct Job {
    pub id: String,
    pub modo: String,
    pub pregunta: String,
    pub plan_json: Option<String>,
    pub status: EstadoJob,
    pub close_reason: Option<MotivoCierre>,
    pub costo_acumulado: f64,
    pub max_cost: Option<f64>,
    pub max_llm_calls: Option<i64>,
    pub config_snapshot: String,
    pub corpus_snapshot_id: Option<String>,
    pub project: String,
    pub corpus: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Un stage del DAG de una investigación.
#[derive(Debug, Clone)]
pub struct Stage {
    pub id: String,
    pub job_id: String,
    pub tipo: String,
    pub titulo: Option<String>,
    pub status: String,
    pub artifact_path: Option<String>,
    pub checkpoint: Option<String>,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
}

/// Configuración para crear un job.
pub struct ConfigJob {
    pub modo: String,
    pub pregunta: String,
    pub project: String,
    pub corpus: String,
    pub config_snapshot: String,
    pub corpus_snapshot_id: Option<String>,
    pub max_cost: Option<f64>,
    pub max_llm_calls: Option<i64>,
}

impl ConfigJob {
    pub fn nueva(pregunta: &str, project: &str) -> Self {
        Self {
            modo: "investigacion".into(),
            pregunta: pregunta.into(),
            project: project.into(),
            corpus: "soip".into(),
            config_snapshot: "{}".into(),
            corpus_snapshot_id: None,
            max_cost: None,
            max_llm_calls: None,
        }
    }
}

/// Datos de una llamada LLM para el ledger y la atribución de costo.
#[derive(Debug, Clone)]
pub struct LlamadaInfo {
    pub rol: String,
    pub modelo: String,
    pub tokens_input: i64,
    pub tokens_output: i64,
    pub costo: f64,
    pub latencia_ms: i64,
    pub reintentos: i64,
    pub error: Option<String>,
}

impl LlamadaInfo {
    pub fn nueva(rol: &str, modelo: &str, costo: f64) -> Self {
        Self {
            rol: rol.into(),
            modelo: modelo.into(),
            tokens_input: 0,
            tokens_output: 0,
            costo,
            latencia_ms: 0,
            reintentos: 0,
            error: None,
        }
    }
}

/// Motor de trabajos sobre `EstadoDb`.
pub struct MotorTrabajos<'a> {
    db: &'a EstadoDb,
}

impl<'a> MotorTrabajos<'a> {
    pub fn nuevo(db: &'a EstadoDb) -> Self {
        Self { db }
    }

    // ── jobs ──────────────────────────────────────────────────────────────

    /// Crea un job en estado `planned`, congelando el snapshot de
    /// configuración (PLAN §6.9) y el snapshot lógico del corpus.
    pub fn crear_job(&self, cfg: ConfigJob) -> Result<Job, String> {
        let id = nuevo_id("job");
        let t = ahora();
        self.db
            .conn()
            .execute(
                "INSERT INTO jobs (id, modo, pregunta, plan_json, status, close_reason, \
             costo_acumulado, max_cost, max_llm_calls, config_snapshot, corpus_snapshot_id, \
             project, corpus, created_at, updated_at) \
             VALUES (?1, ?2, ?3, NULL, 'planned', NULL, 0, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
                params![
                    id,
                    cfg.modo,
                    cfg.pregunta,
                    cfg.max_cost,
                    cfg.max_llm_calls,
                    cfg.config_snapshot,
                    cfg.corpus_snapshot_id,
                    cfg.project,
                    cfg.corpus,
                    t
                ],
            )
            .map_err(|e| e.to_string())?;
        self.obtener_job(&id)
            .ok_or_else(|| "El job no se pudo leer tras crearlo".into())
    }

    /// Ids de todos los jobs (para reanudar tras un crash).
    pub fn listar_ids_jobs(&self) -> Vec<String> {
        let Ok(mut stmt) = self
            .db
            .conn()
            .prepare("SELECT id FROM jobs ORDER BY created_at")
        else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) else {
            return Vec::new();
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    /// Lee un job por id.
    pub fn obtener_job(&self, id: &str) -> Option<Job> {
        let mut stmt = self
            .db
            .conn()
            .prepare(
                "SELECT id, modo, pregunta, plan_json, status, close_reason, costo_acumulado, \
                 max_cost, max_llm_calls, config_snapshot, corpus_snapshot_id, project, corpus, \
                 created_at, updated_at FROM jobs WHERE id = ?1",
            )
            .ok()?;
        stmt.query_row(params![id], |r| {
            let status: String = r.get(4)?;
            let close: Option<String> = r.get(5)?;
            Ok(Job {
                id: r.get(0)?,
                modo: r.get(1)?,
                pregunta: r.get(2)?,
                plan_json: r.get(3)?,
                status: EstadoJob::desde_str(&status).unwrap_or(EstadoJob::Failed),
                close_reason: close.as_deref().and_then(MotivoCierre::desde_str),
                costo_acumulado: r.get(6)?,
                max_cost: r.get(7)?,
                max_llm_calls: r.get(8)?,
                config_snapshot: r.get(9)?,
                corpus_snapshot_id: r.get(10)?,
                project: r.get(11)?,
                corpus: r.get(12)?,
                created_at: r.get(13)?,
                updated_at: r.get(14)?,
            })
        })
        .optional()
        .ok()
        .flatten()
    }

    /// Persiste el plan del orquestador (JSON) en el job.
    pub fn guardar_plan(&self, job_id: &str, plan_json: &str) -> Result<(), String> {
        self.db
            .conn()
            .execute(
                "UPDATE jobs SET plan_json = ?1, updated_at = ?2 WHERE id = ?3",
                params![plan_json, ahora(), job_id],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Transición de estado con validación mínima y evento.
    fn transicionar(&self, job_id: &str, nuevo: EstadoJob, evento: &str) -> Result<(), String> {
        self.db
            .conn()
            .execute(
                "UPDATE jobs SET status = ?1, updated_at = ?2 WHERE id = ?3",
                params![nuevo.as_str(), ahora(), job_id],
            )
            .map_err(|e| e.to_string())?;
        self.registrar_evento(job_id, None, evento, None)
    }

    /// Cierra el job con un motivo. `blocked` → estado `failed`; el resto → `done`.
    pub fn cerrar(&self, job_id: &str, motivo: MotivoCierre) -> Result<(), String> {
        let estado = if motivo == MotivoCierre::Blocked {
            EstadoJob::Failed
        } else {
            EstadoJob::Done
        };
        self.db
            .conn()
            .execute(
                "UPDATE jobs SET status = ?1, close_reason = ?2, updated_at = ?3 WHERE id = ?4",
                params![estado.as_str(), motivo.as_str(), ahora(), job_id],
            )
            .map_err(|e| e.to_string())?;
        self.registrar_evento(
            job_id,
            None,
            "job_closed",
            Some(&format!("{{\"close_reason\":\"{}\"}}", motivo.as_str())),
        )
    }

    /// Estados de transición del ciclo de vida.
    pub fn pausar(&self, job_id: &str) -> Result<(), String> {
        self.transicionar(job_id, EstadoJob::Paused, "job_paused")
    }

    pub fn reanudar(&self, job_id: &str) -> Result<(), String> {
        self.transicionar(job_id, EstadoJob::Running, "job_resumed")
    }

    pub fn poner_en_marcha(&self, job_id: &str) -> Result<(), String> {
        self.transicionar(job_id, EstadoJob::Running, "job_started")
    }

    // ── stages y DAG ──────────────────────────────────────────────────────

    /// Crea un stage en el job y devuelve su id.
    pub fn agregar_stage(&self, job_id: &str, tipo: &str, titulo: &str) -> Result<String, String> {
        let id = nuevo_id("stage");
        self.db
            .conn()
            .execute(
                "INSERT INTO stages (id, job_id, tipo, titulo, status, started_at, completed_at) \
                 VALUES (?1, ?2, ?3, ?4, 'pending', NULL, NULL)",
                params![id, job_id, tipo, titulo],
            )
            .map_err(|e| e.to_string())?;
        Ok(id)
    }

    /// Declara que `stage_id` depende de `depende_de`.
    pub fn agregar_dependencia(&self, stage_id: &str, depende_de: &str) -> Result<(), String> {
        self.db
            .conn()
            .execute(
                "INSERT OR IGNORE INTO stage_dependencies (stage_id, depends_on_stage_id) \
                 VALUES (?1, ?2)",
                params![stage_id, depende_de],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn stages_del_job(&self, job_id: &str) -> Vec<Stage> {
        let Ok(mut stmt) = self.db.conn().prepare(
            "SELECT id, job_id, tipo, titulo, status, artifact_path, checkpoint, \
             started_at, completed_at FROM stages WHERE job_id = ?1",
        ) else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map(params![job_id], |r| {
            Ok(Stage {
                id: r.get(0)?,
                job_id: r.get(1)?,
                tipo: r.get(2)?,
                titulo: r.get(3)?,
                status: r.get(4)?,
                artifact_path: r.get(5)?,
                checkpoint: r.get(6)?,
                started_at: r.get(7)?,
                completed_at: r.get(8)?,
            })
        }) else {
            return Vec::new();
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    fn aristas_del_job(&self, job_id: &str) -> Vec<grafo::Arista> {
        let Ok(mut stmt) = self.db.conn().prepare(
            "SELECT sd.stage_id, sd.depends_on_stage_id FROM stage_dependencies sd \
             JOIN stages s ON s.id = sd.stage_id WHERE s.job_id = ?1",
        ) else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map(params![job_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        }) else {
            return Vec::new();
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    /// Valida que el DAG del job no tenga ciclos y devuelve el orden
    /// topológico de ejecución (PLAN §6.2). `Err` trae los nodos en ciclo.
    pub fn validar_dag(&self, job_id: &str) -> Result<Vec<String>, Vec<String>> {
        let stages = self.stages_del_job(job_id);
        let nodos: Vec<String> = stages.iter().map(|s| s.id.clone()).collect();
        let aristas = self.aristas_del_job(job_id);
        grafo::orden_topologico(&nodos, &aristas)
    }

    /// Dependencias transitivas de un stage (nodos de los que depende, directa
    /// o indirectamente).
    fn dependencias_transitivas(&self, job_id: &str, stage_id: &str) -> Vec<String> {
        // Arista (stage, depende_de): el stage apunta a su dependencia.
        let aristas = self.aristas_del_job(job_id);
        let mut dependencias = std::collections::HashMap::<String, Vec<String>>::new();
        for (stage, dep) in &aristas {
            dependencias
                .entry(stage.clone())
                .or_default()
                .push(dep.clone());
        }
        let mut vistos = std::collections::HashSet::new();
        let mut pila = vec![stage_id.to_string()];
        while let Some(nodo) = pila.pop() {
            if let Some(deps) = dependencias.get(&nodo) {
                for d in deps {
                    if vistos.insert(d.clone()) {
                        pila.push(d.clone());
                    }
                }
            }
        }
        vistos.into_iter().collect()
    }

    /// Siguiente stage a ejecutar (PLAN §6.7): primero el que quedó
    /// `in_progress` de una ejecución interrumpida (crash-safe), luego el
    /// primer `pending` cuyo orden topológico permita ejecutarlo.
    pub fn siguiente_stage(&self, job_id: &str) -> Result<Option<Stage>, String> {
        let stages = self.stages_del_job(job_id);
        // Reanudar: un stage a medio hacer se retoma (o se repite).
        if let Some(s) = stages.iter().find(|s| s.status == "in_progress") {
            return Ok(Some(s.clone()));
        }
        let orden = self
            .validar_dag(job_id)
            .map_err(|ciclo| format!("El DAG del job {job_id} tiene un ciclo: {ciclo:?}"))?;
        for id in orden {
            let stage = stages.iter().find(|s| s.id == id).unwrap();
            if stage.status != "pending" {
                continue;
            }
            // Todas sus dependencias deben estar completas o reutilizadas.
            let deps = self.dependencias_transitivas(job_id, &stage.id);
            let deps_ok = deps.iter().all(|d| {
                stages
                    .iter()
                    .find(|s| s.id == *d)
                    .map(|s| s.status == "completed" || s.status == "reused")
                    .unwrap_or(true)
            });
            if deps_ok {
                return Ok(Some(stage.clone()));
            }
        }
        Ok(None)
    }

    /// Marca el inicio de un stage (checkpoint previo a ejecutar).
    pub fn marcar_inicio_stage(&self, job_id: &str, stage_id: &str) -> Result<(), String> {
        let t = ahora();
        self.db
            .conn()
            .execute(
                "UPDATE stages SET status = 'in_progress', started_at = ?1, checkpoint = ?2 \
                 WHERE id = ?3",
                params![t, format!("in_progress@{t}"), stage_id],
            )
            .map_err(|e| e.to_string())?;
        self.registrar_evento(job_id, Some(stage_id), "stage_started", None)
    }

    /// Completa un stage: persiste el artefacto y la fila de estado antes de
    /// avanzar (checkpoint por stage, crash-safe).
    pub fn completar_stage(
        &self,
        job_id: &str,
        stage_id: &str,
        tipo_artefacto: &str,
        artifact_path: &str,
    ) -> Result<(), String> {
        let t = ahora();
        self.db.con_transaccion(|conn| {
            let artefacto_id = nuevo_id("art");
            conn.execute(
                "INSERT INTO artifacts (id, job_id, tipo, path, padre, version, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
                params![
                    artefacto_id,
                    job_id,
                    tipo_artefacto,
                    artifact_path,
                    stage_id,
                    t
                ],
            )
            .map_err(|e| e.to_string())?;
            conn.execute(
                "UPDATE stages SET status = 'completed', artifact_path = ?1, checkpoint = ?2, \
                 completed_at = ?3 WHERE id = ?4",
                params![
                    artifact_path,
                    format!("completed@{t}:{artefacto_id}"),
                    t,
                    stage_id
                ],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })?;
        self.registrar_evento(
            job_id,
            Some(stage_id),
            "stage_completed",
            Some(&format!("{{\"artifact\":\"{artifact_path}\"}}")),
        )
    }

    /// Marca un stage como reutilizado (PLAN §6.2: snapshots iguales + ninguna
    /// dependencia terminó después de él).
    pub fn marcar_reutilizado(&self, job_id: &str, stage_id: &str) -> Result<(), String> {
        self.db
            .conn()
            .execute(
                "UPDATE stages SET status = 'reused', checkpoint = ?1 WHERE id = ?2",
                params![format!("reused@{}", ahora()), stage_id],
            )
            .map_err(|e| e.to_string())?;
        self.registrar_evento(job_id, Some(stage_id), "stage_reused", None)
    }

    /// Un stage es reutilizable si (a) los snapshots del job no cambiaron y
    /// (b) ninguna dependencia directa o transitiva terminó después de que él
    /// terminó (§6.2).
    pub fn stage_reutilizable(&self, job_id: &str, stage_id: &str) -> Result<bool, String> {
        let job = self
            .obtener_job(job_id)
            .ok_or_else(|| format!("Job inexistente: {job_id}"))?;
        let stages = self.stages_del_job(job_id);
        let stage = stages
            .iter()
            .find(|s| s.id == stage_id)
            .ok_or_else(|| format!("Stage inexistente: {stage_id}"))?;
        let Some(completado) = stage.completed_at else {
            return Ok(false);
        };
        // (a) Los snapshots se congelan al crear el job; una re-apertura con el
        // mismo job implica config/snapshot idénticos. Se verifica que existan.
        if job.config_snapshot.is_empty() || job.corpus_snapshot_id.is_none() {
            return Ok(false);
        }
        // (b) Ninguna dependencia (directa o transitiva) terminó después.
        let deps = self.dependencias_transitivas(job_id, stage_id);
        for d in deps {
            if let Some(dep) = stages.iter().find(|s| s.id == d) {
                if let Some(t) = dep.completed_at {
                    if t > completado {
                        return Ok(false);
                    }
                }
            }
        }
        Ok(true)
    }

    // ── ledger: queries, llm_calls, eventos, decisiones ───────────────────

    /// Registra una consulta en el log (memoria de búsqueda, §6.2).
    pub fn registrar_consulta(
        &self,
        job_id: &str,
        stage_id: Option<&str>,
        consulta: &str,
        filtros: Option<&str>,
        resultados: Option<i64>,
        reformulated_from: Option<&str>,
    ) -> Result<String, String> {
        let id = nuevo_id("q");
        self.db
            .conn()
            .execute(
                "INSERT INTO queries (id, job_id, stage_id, consulta, filtros, resultados, \
                 reformulated_from, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    id,
                    job_id,
                    stage_id,
                    consulta,
                    filtros,
                    resultados,
                    reformulated_from,
                    ahora()
                ],
            )
            .map_err(|e| e.to_string())?;
        Ok(id)
    }

    /// Registra una llamada LLM, acumula el costo y aplica los budgets duros.
    /// Devuelve `true` si el job quedó cerrado por presupuesto.
    pub fn registrar_llamada(
        &self,
        job_id: &str,
        stage_id: Option<&str>,
        info: &LlamadaInfo,
    ) -> Result<bool, String> {
        let id = nuevo_id("llm");
        self.db.con_transaccion(|conn| {
            conn.execute(
                "INSERT INTO llm_calls (id, job_id, stage_id, rol, modelo, tokens_input, \
                 tokens_output, costo, latencia_ms, reintentos, error, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    id, job_id, stage_id, info.rol, info.modelo, info.tokens_input,
                    info.tokens_output, info.costo, info.latencia_ms, info.reintentos,
                    info.error, ahora()
                ],
            )
            .map_err(|e| e.to_string())?;
            conn.execute(
                "UPDATE jobs SET costo_acumulado = costo_acumulado + ?1, updated_at = ?2 WHERE id = ?3",
                params![info.costo, ahora(), job_id],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })?;
        let llamadas = self.contar_llm_calls(job_id);
        let costo_total = self
            .db
            .conn()
            .query_row(
                "SELECT costo_acumulado FROM jobs WHERE id = ?1",
                params![job_id],
                |r| r.get(0),
            )
            .unwrap_or(0.0);

        let job = self.obtener_job(job_id).ok_or("job perdido")?;
        let excede_costo = job.max_cost.map(|m| costo_total >= m).unwrap_or(false);
        let excede_llamadas = job.max_llm_calls.map(|m| llamadas >= m).unwrap_or(false);
        if excede_costo || excede_llamadas {
            self.cerrar(job_id, MotivoCierre::BudgetExhausted)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Cuenta las llamadas LLM de un job.
    pub fn contar_llm_calls(&self, job_id: &str) -> i64 {
        self.db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM llm_calls WHERE job_id = ?1",
                params![job_id],
                |r| r.get(0),
            )
            .unwrap_or(0)
    }

    /// Registra un evento de progreso (append-only, base del timeline Tauri).
    pub fn registrar_evento(
        &self,
        job_id: &str,
        stage_id: Option<&str>,
        tipo: &str,
        payload: Option<&str>,
    ) -> Result<(), String> {
        let id = nuevo_id("ev");
        self.db
            .conn()
            .execute(
                "INSERT INTO job_events (id, job_id, stage_id, tipo, payload, timestamp) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, job_id, stage_id, tipo, payload, ahora()],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Lista los eventos de un job en orden.
    pub fn listar_eventos(&self, job_id: &str) -> Vec<(String, Option<String>, String, i64)> {
        let Ok(mut stmt) = self.db.conn().prepare(
            "SELECT tipo, stage_id, COALESCE(payload, ''), timestamp FROM job_events \
                 WHERE job_id = ?1 ORDER BY timestamp, rowid",
        ) else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map(params![job_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
            ))
        }) else {
            return Vec::new();
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    /// Pasa el job a `awaiting_human` con el alcance y el costo estimado del
    /// gate (PLAN §6.7: antes de una pasada masiva o de cerrar el informe).
    pub fn esperar_humano(
        &self,
        job_id: &str,
        stage_id: Option<&str>,
        alcance: &str,
        costo_estimado: f64,
    ) -> Result<(), String> {
        self.transicionar(job_id, EstadoJob::AwaitingHuman, "awaiting_human")?;
        self.db
            .conn()
            .execute(
                "UPDATE jobs SET updated_at = ?1 WHERE id = ?2",
                params![ahora(), job_id],
            )
            .map_err(|e| e.to_string())?;
        // El alcance y el costo quedan persistidos en human_decisions (pendiente
        // de decisión) para que tras un crash quede claro qué fue autorizado.
        let id = nuevo_id("dec");
        self.db
            .conn()
            .execute(
                "INSERT INTO human_decisions (id, job_id, stage_id, alcance, costo_estimado, \
                 decision, timestamp) VALUES (?1, ?2, ?3, ?4, ?5, 'pendiente', ?6)",
                params![id, job_id, stage_id, alcance, costo_estimado, ahora()],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Registra la decisión humana de un gate y vuelve el job a `running`.
    pub fn registrar_decision_humana(
        &self,
        job_id: &str,
        stage_id: Option<&str>,
        decision: &str,
    ) -> Result<(), String> {
        self.db
            .conn()
            .execute(
                "UPDATE human_decisions SET decision = ?1, timestamp = ?2 \
                 WHERE job_id = ?3 AND (stage_id IS ?4 OR (?4 IS NULL AND stage_id IS NULL))",
                params![decision, ahora(), job_id, stage_id],
            )
            .map_err(|e| e.to_string())?;
        self.transicionar(job_id, EstadoJob::Running, "job_resumed")?;
        self.registrar_evento(
            job_id,
            stage_id,
            "human_decision",
            Some(&format!("{{\"decision\":\"{decision}\"}}")),
        )
    }

    /// Lista los stages de un job.
    pub fn stages(&self, job_id: &str) -> Vec<Stage> {
        self.stages_del_job(job_id)
    }
}

impl MotivoCierre {
    fn desde_str(s: &str) -> Option<Self> {
        match s {
            "completed" => Some(Self::Completed),
            "cancelled" => Some(Self::Cancelled),
            "budget_exhausted" => Some(Self::BudgetExhausted),
            "blocked" => Some(Self::Blocked),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crear_job_basico(db: &EstadoDb) -> Job {
        let m = MotorTrabajos::nuevo(db);
        m.crear_job(ConfigJob {
            modo: "investigacion".into(),
            pregunta: "¿Conflictividad SOIP?".into(),
            project: "soip-conflictividad".into(),
            corpus: "soip".into(),
            config_snapshot: "{\"modelo\":\"test\",\"denylist\":[]}".into(),
            corpus_snapshot_id: Some("snap-1".into()),
            max_cost: None,
            max_llm_calls: None,
        })
        .unwrap()
    }

    #[test]
    fn el_job_se_crea_planned_con_snapshot_congelado() {
        let db = EstadoDb::abrir_en_memoria().unwrap();
        let job = crear_job_basico(&db);
        assert_eq!(job.status, EstadoJob::Planned);
        assert_eq!(job.config_snapshot, "{\"modelo\":\"test\",\"denylist\":[]}");
        assert_eq!(job.corpus_snapshot_id.as_deref(), Some("snap-1"));
        assert_eq!(job.pregunta, "¿Conflictividad SOIP?");
    }

    #[test]
    fn el_dag_rechaza_ciclos() {
        let db = EstadoDb::abrir_en_memoria().unwrap();
        let m = MotorTrabajos::nuevo(&db);
        let job = crear_job_basico(&db);
        let a = m.agregar_stage(&job.id, "consulta", "A").unwrap();
        let b = m.agregar_stage(&job.id, "consulta", "B").unwrap();
        m.agregar_dependencia(&a, &b).unwrap();
        m.agregar_dependencia(&b, &a).unwrap();
        let err = m.validar_dag(&job.id).unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn el_orden_topologico_guia_la_ejecucion() {
        let db = EstadoDb::abrir_en_memoria().unwrap();
        let m = MotorTrabajos::nuevo(&db);
        let job = crear_job_basico(&db);
        let a = m.agregar_stage(&job.id, "consulta", "Cronología").unwrap();
        let b = m.agregar_stage(&job.id, "consulta", "Actores").unwrap();
        let c = m.agregar_stage(&job.id, "sintesis", "Síntesis").unwrap();
        m.agregar_dependencia(&b, &a).unwrap();
        m.agregar_dependencia(&c, &a).unwrap();
        m.agregar_dependencia(&c, &b).unwrap();
        let orden = m.validar_dag(&job.id).unwrap();
        let pos = |id: &str| orden.iter().position(|x| x == id).unwrap();
        assert!(pos(&a) < pos(&b));
        assert!(pos(&b) < pos(&c));
        assert!(pos(&a) < pos(&c));

        // El primer stage a ejecutar es A (sin dependencias).
        let siguiente = m.siguiente_stage(&job.id).unwrap().unwrap();
        assert_eq!(siguiente.id, a);
    }

    #[test]
    fn completar_un_stage_abre_el_siguiente() {
        let db = EstadoDb::abrir_en_memoria().unwrap();
        let m = MotorTrabajos::nuevo(&db);
        let job = crear_job_basico(&db);
        let a = m.agregar_stage(&job.id, "consulta", "A").unwrap();
        let b = m.agregar_stage(&job.id, "consulta", "B").unwrap();
        m.agregar_dependencia(&b, &a).unwrap();

        m.marcar_inicio_stage(&job.id, &a).unwrap();
        m.completar_stage(&job.id, &a, "sintesis", "sintesis/a.md")
            .unwrap();
        let siguiente = m.siguiente_stage(&job.id).unwrap().unwrap();
        assert_eq!(siguiente.id, b);
        // El artefacto quedó registrado.
        let artefactos: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM artifacts WHERE job_id = ?1",
                params![job.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(artefactos, 1);
    }

    #[test]
    fn el_resume_retoma_el_stage_interrumpido() {
        let db = EstadoDb::abrir_en_memoria().unwrap();
        let m = MotorTrabajos::nuevo(&db);
        let job = crear_job_basico(&db);
        let a = m.agregar_stage(&job.id, "consulta", "A").unwrap();
        let b = m.agregar_stage(&job.id, "consulta", "B").unwrap();
        m.agregar_dependencia(&b, &a).unwrap();

        // Simula un crash: A quedó en in_progress sin completar.
        m.marcar_inicio_stage(&job.id, &a).unwrap();
        // Reabre la base (proceso nuevo) y retoma.
        let m2 = MotorTrabajos::nuevo(&db);
        let siguiente = m2.siguiente_stage(&job.id).unwrap().unwrap();
        assert_eq!(siguiente.id, a);
        assert_eq!(siguiente.status, "in_progress");
    }

    #[test]
    fn los_eventos_son_append_only() {
        let db = EstadoDb::abrir_en_memoria().unwrap();
        let m = MotorTrabajos::nuevo(&db);
        let job = crear_job_basico(&db);
        m.registrar_evento(&job.id, None, "prueba", Some("{\"n\":1}"))
            .unwrap();
        m.registrar_evento(&job.id, None, "prueba", Some("{\"n\":2}"))
            .unwrap();
        let eventos = m.listar_eventos(&job.id);
        assert_eq!(eventos.len(), 2);
        assert_eq!(eventos[0].0, "prueba");
    }

    #[test]
    fn agotar_max_llm_calls_cierra_por_presupuesto() {
        let db = EstadoDb::abrir_en_memoria().unwrap();
        let m = MotorTrabajos::nuevo(&db);
        let job = m
            .crear_job(ConfigJob {
                modo: "investigacion".into(),
                pregunta: "p".into(),
                project: "p".into(),
                corpus: "soip".into(),
                config_snapshot: "{}".into(),
                corpus_snapshot_id: Some("s".into()),
                max_cost: None,
                max_llm_calls: Some(2),
            })
            .unwrap();
        let llamada = LlamadaInfo {
            costo: 0.001,
            ..LlamadaInfo::nueva("worker", "m", 0.0)
        };
        m.registrar_llamada(&job.id, None, &llamada).unwrap();
        let cerrado = m.registrar_llamada(&job.id, None, &llamada).unwrap();
        assert!(cerrado);
        let job_final = m.obtener_job(&job.id).unwrap();
        assert_eq!(job_final.status, EstadoJob::Done);
        assert_eq!(job_final.close_reason, Some(MotivoCierre::BudgetExhausted));
    }

    #[test]
    fn agotar_max_cost_cierra_por_presupuesto() {
        let db = EstadoDb::abrir_en_memoria().unwrap();
        let m = MotorTrabajos::nuevo(&db);
        let job = m
            .crear_job(ConfigJob {
                modo: "investigacion".into(),
                pregunta: "p".into(),
                project: "p".into(),
                corpus: "soip".into(),
                config_snapshot: "{}".into(),
                corpus_snapshot_id: Some("s".into()),
                max_cost: Some(0.01),
                max_llm_calls: None,
            })
            .unwrap();
        let llamada = LlamadaInfo {
            costo: 0.02,
            ..LlamadaInfo::nueva("worker", "m", 0.0)
        };
        let cerrado = m.registrar_llamada(&job.id, None, &llamada).unwrap();
        assert!(cerrado);
        let job_final = m.obtener_job(&job.id).unwrap();
        assert_eq!(job_final.close_reason, Some(MotivoCierre::BudgetExhausted));
    }

    #[test]
    fn el_gate_humano_persiste_la_decision() {
        let db = EstadoDb::abrir_en_memoria().unwrap();
        let m = MotorTrabajos::nuevo(&db);
        let job = crear_job_basico(&db);
        m.esperar_humano(&job.id, None, "pasada masiva de 500 consultas", 12.5)
            .unwrap();
        let job_esperando = m.obtener_job(&job.id).unwrap();
        assert_eq!(job_esperando.status, EstadoJob::AwaitingHuman);
        let decisiones: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM human_decisions WHERE job_id = ?1 AND decision = 'pendiente'",
                params![job.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(decisiones, 1);
        m.registrar_decision_humana(&job.id, None, "aprobado")
            .unwrap();
        let job_ok = m.obtener_job(&job.id).unwrap();
        assert_eq!(job_ok.status, EstadoJob::Running);
    }

    #[test]
    fn reutilizacion_por_snapshots_y_timestamps() {
        let db = EstadoDb::abrir_en_memoria().unwrap();
        let m = MotorTrabajos::nuevo(&db);
        let job = crear_job_basico(&db);
        let a = m.agregar_stage(&job.id, "consulta", "A").unwrap();
        let b = m.agregar_stage(&job.id, "consulta", "B").unwrap();
        m.agregar_dependencia(&b, &a).unwrap();

        m.marcar_inicio_stage(&job.id, &a).unwrap();
        m.completar_stage(&job.id, &a, "sintesis", "a.md").unwrap();
        m.marcar_inicio_stage(&job.id, &b).unwrap();
        m.completar_stage(&job.id, &b, "sintesis", "b.md").unwrap();

        // Con los snapshots intactos, ambos stages son reutilizables.
        assert!(m.stage_reutilizable(&job.id, &a).unwrap());
        assert!(m.stage_reutilizable(&job.id, &b).unwrap());

        // Si A se rehace después (completed_at posterior), B queda vencido.
        let t = ahora();
        db.conn()
            .execute(
                "UPDATE stages SET completed_at = ?1, status = 'completed' WHERE id = ?2",
                params![t + 1000, a],
            )
            .unwrap();
        assert!(!m.stage_reutilizable(&job.id, &b).unwrap());
        assert!(m.stage_reutilizable(&job.id, &a).unwrap());
    }

    #[test]
    fn las_consultas_quedan_en_el_log() {
        let db = EstadoDb::abrir_en_memoria().unwrap();
        let m = MotorTrabajos::nuevo(&db);
        let job = crear_job_basico(&db);
        let q1 = m
            .registrar_consulta(&job.id, None, "huelga 1965", None, Some(12), None)
            .unwrap();
        let q2 = m
            .registrar_consulta(&job.id, None, "conflicto pesca", None, Some(8), Some(&q1))
            .unwrap();
        let filas: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM queries WHERE job_id = ?1",
                params![job.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(filas, 2);
        let reformulada: String = db
            .conn()
            .query_row(
                "SELECT reformulated_from FROM queries WHERE id = ?1",
                params![q2],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reformulada, q1);
    }
}

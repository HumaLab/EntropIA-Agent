//! Job API pública (PLAN §2, Fase 4): el seam de integración con EntropIA
//! Lite/Pro. Hoy la envuelve el CLI; mañana la envuelven commands Tauri con
//! eventos de progreso (`research_start / research_step / research_pause /
//! research_resume / research_status / research_events / research_artifacts`).
//! No hay que rediseñar para integrar.
//!
//! `estado.sqlite` es gestionado por el desktop (archivo separado en el dir de
//! datos de la app, decisión §11#1): la frontera read-only se mantiene a nivel
//! de archivo — el agente nunca abre el corpus para escribir.

use std::path::PathBuf;

use crate::cliente_llm::ClienteLlm;
use crate::estado::EstadoDb;
use crate::orquestador::MotorInvestigacion;
use crate::repositorio::RepositorioSqlite;
use crate::trabajos::{EstadoJob, Job, MotivoCierre, MotorTrabajos};

/// Progreso devuelto por `research_step`.
#[derive(Debug, Clone)]
pub struct ProgresoInvest {
    pub job_id: String,
    pub stage_ejecutado: Option<String>,
    pub quedan_stages: bool,
    pub terminado: bool,
    pub informe_path: Option<String>,
}

/// Info de un artefacto del job (para la UI).
#[derive(Debug, Clone)]
pub struct ArtifactoJob {
    pub id: String,
    pub tipo: String,
    pub path: String,
    pub padre: Option<String>,
    pub version: i64,
}

/// API de integración sobre el motor de investigación.
pub struct ApiAgente<'a> {
    pub llm: &'a dyn ClienteLlm,
    pub repo: &'a RepositorioSqlite,
    pub db: &'a EstadoDb,
    pub dir_artefactos: PathBuf,
}

impl<'a> ApiAgente<'a> {
    fn inv(&self) -> MotorInvestigacion<'a> {
        MotorInvestigacion {
            llm: self.llm,
            repo: self.repo,
            recuperador: None,
            motor: MotorTrabajos::nuevo(self.db),
            ledger: crate::dominio::Ledger::nuevo(self.db),
            memoria: crate::memoria::MemoriaDb::nuevo(self.db),
            dir_artefactos: self.dir_artefactos.clone(),
            db: self.db,
        }
    }

    /// `research_start`: crea el job (snapshot congelado) y prepara el plan.
    pub fn research_start(
        &self,
        pedido: &str,
        project: &str,
        config_snapshot: &str,
        max_cost: Option<f64>,
        max_llm_calls: Option<i64>,
    ) -> Result<String, String> {
        let inv = self.inv();
        let corpus_snapshot = self.repo.snapshot_corpus()?;
        let job = inv.motor.crear_job(crate::trabajos::ConfigJob {
            modo: "investigacion".into(),
            pregunta: pedido.into(),
            project: project.into(),
            corpus: "soip".into(),
            config_snapshot: config_snapshot.into(),
            corpus_snapshot_id: Some(corpus_snapshot),
            max_cost,
            max_llm_calls,
        })?;
        inv.preparar_job(&job.id, pedido)?;
        Ok(job.id)
    }

    /// `research_step`: ejecuta un stage (checkpoint entre medio). Cuando no
    /// quedan stages, ensambla el informe y cierra el job.
    pub fn research_step(&self, job_id: &str) -> Result<ProgresoInvest, String> {
        let inv = self.inv();
        let stage = inv.motor.siguiente_stage(job_id)?.map(|s| s.id.clone());
        let quedan = inv.ejecutar_siguiente_stage(job_id)?;
        if quedan {
            return Ok(ProgresoInvest {
                job_id: job_id.into(),
                stage_ejecutado: stage,
                quedan_stages: true,
                terminado: false,
                informe_path: None,
            });
        }
        // No quedan stages: cierre con informe.
        let informe_path = inv.cerrar_con_informe(job_id)?;
        Ok(ProgresoInvest {
            job_id: job_id.into(),
            stage_ejecutado: stage,
            quedan_stages: false,
            terminado: true,
            informe_path: Some(informe_path.to_string_lossy().into_owned()),
        })
    }

    /// `research_pause` / `research_resume`.
    pub fn research_pause(&self, job_id: &str) -> Result<(), String> {
        MotorTrabajos::nuevo(self.db).pausar(job_id)
    }

    pub fn research_resume(&self, job_id: &str) -> Result<(), String> {
        MotorTrabajos::nuevo(self.db).reanudar(job_id)
    }

    /// `research_status`: estado del job y de sus stages.
    pub fn research_status(&self, job_id: &str) -> Result<Job, String> {
        MotorTrabajos::nuevo(self.db)
            .obtener_job(job_id)
            .ok_or_else(|| format!("Job inexistente: {job_id}"))
    }

    /// `research_events`: eventos de progreso del job (append-only).
    pub fn research_events(&self, job_id: &str) -> Result<Vec<String>, String> {
        let eventos = MotorTrabajos::nuevo(self.db).listar_eventos(job_id);
        Ok(eventos
            .into_iter()
            .map(|(tipo, stage, payload, ts)| {
                format!(
                    "[{ts}] {tipo}{} {payload}",
                    stage.map(|s| format!(" ({s})")).unwrap_or_default()
                )
            })
            .collect())
    }

    /// `research_artifacts`: artefactos del job (síntesis, secciones, informe).
    pub fn research_artifacts(&self, job_id: &str) -> Result<Vec<ArtifactoJob>, String> {
        let Ok(mut stmt) = self.db.conn().prepare(
            "SELECT id, tipo, path, padre, version FROM artifacts WHERE job_id = ?1 \
             ORDER BY rowid",
        ) else {
            return Ok(Vec::new());
        };
        let Ok(rows) = stmt.query_map(rusqlite::params![job_id], |r| {
            Ok(ArtifactoJob {
                id: r.get(0)?,
                tipo: r.get(1)?,
                path: r.get(2)?,
                padre: r.get(3)?,
                version: r.get(4)?,
            })
        }) else {
            return Ok(Vec::new());
        };
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// `research_mostrar_fuente`: ruta del asset original + página para abrir
    /// el escaneo real en el desktop.
    pub fn research_mostrar_fuente(&self, item_id: &str) -> Vec<(String, Option<i64>)> {
        crate::puerta_lectura::mostrar_fuente(self.repo, item_id)
    }

    /// ¿El job puede retomarse tras un crash? (resume crash-safe, §6.7).
    pub fn hay_jobs_reanudables(&self) -> Vec<String> {
        MotorTrabajos::nuevo(self.db)
            .listar_ids_jobs()
            .into_iter()
            .filter(|id| {
                self.research_status(id)
                    .map(|j| {
                        matches!(
                            j.status,
                            EstadoJob::Running
                                | EstadoJob::Paused
                                | EstadoJob::AwaitingHuman
                                | EstadoJob::Planned
                        )
                    })
                    .unwrap_or(false)
            })
            .collect()
    }

    /// Cierra un job con motivo (por ejemplo, cancelación desde la UI).
    pub fn research_cancelar(&self, job_id: &str) -> Result<(), String> {
        MotorTrabajos::nuevo(self.db).cerrar(job_id, MotivoCierre::Cancelled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::estado::EstadoDb;
    use crate::llm_fake::LlmSintetizaEvidencia;
    use crate::repositorio::RepositorioSqlite;
    use crate::trabajos::EstadoJob;

    fn base() -> (EstadoDb, RepositorioSqlite, std::path::PathBuf) {
        let repo =
            RepositorioSqlite::abrir(crate::tests_comunes::corpus_sintetico().to_str().unwrap())
                .unwrap();
        let db = EstadoDb::abrir_en_memoria().unwrap();
        // Directorio único por test: los tests corren en paralelo y comparten
        // el pid.
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
        llm: &'a LlmSintetizaEvidencia,
        db: &'a EstadoDb,
        repo: &'a RepositorioSqlite,
        dir: &'a std::path::Path,
    ) -> ApiAgente<'a> {
        ApiAgente {
            llm,
            repo,
            db,
            dir_artefactos: dir.to_path_buf(),
        }
    }

    #[test]
    fn el_ciclo_start_step_pause_resume_status_events_artifacts() {
        let (db, repo, dir) = base();
        let llm = LlmSintetizaEvidencia;
        let api = api(&llm, &db, &repo, &dir);

        // research_start: crea el job y prepara el plan.
        let job_id = api
            .research_start(
                "¿Conflictividad en el SOIP?",
                "soip-conflictividad",
                "{\"modelo\":\"fake\"}",
                None,
                None,
            )
            .unwrap();
        let estado = api.research_status(&job_id).unwrap();
        assert_eq!(estado.status, EstadoJob::Running);
        assert!(
            estado.plan_json.is_some(),
            "el plan debe persistirse al iniciar"
        );

        // research_step: un stage por llamada hasta terminar. Con el contrato
        // correcto de ejecutar_siguiente_stage, el último paso cierra el job:
        // no hay pasos fantasma.
        let mut pasos = 0;
        loop {
            let progreso = api.research_step(&job_id).unwrap();
            pasos += 1;
            if progreso.terminado {
                assert!(progreso.informe_path.is_some());
                assert!(std::path::Path::new(progreso.informe_path.as_deref().unwrap()).exists());
                break;
            }
            assert!(pasos < 10, "el DAG debe converger en pocos pasos");
        }
        assert_eq!(pasos, 4, "4 stages del plan → 4 steps (el último cierra)");
        let estado = api.research_status(&job_id).unwrap();
        assert_eq!(estado.status, EstadoJob::Done);
        assert_eq!(estado.close_reason, Some(MotivoCierre::Completed));

        // research_events: el timeline quedó registrado.
        let eventos = api.research_events(&job_id).unwrap();
        assert!(eventos.iter().any(|e| e.contains("stage_started")));
        assert!(eventos.iter().any(|e| e.contains("informe_ensamblado")));

        // research_artifacts: secciones versionadas + informe.
        let artefactos = api.research_artifacts(&job_id).unwrap();
        assert!(artefactos.iter().any(|a| a.tipo == "seccion"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pausa_y_reanudacion_a_mitad_de_job_con_continuidad_de_checkpoint() {
        let (db, repo, dir) = base();
        let llm = LlmSintetizaEvidencia;
        let api = api(&llm, &db, &repo, &dir);

        // start → primer step.
        let job_id = api
            .research_start(
                "¿Conflictividad en el SOIP?",
                "soip-conflictividad",
                "{}",
                None,
                None,
            )
            .unwrap();
        let p1 = api.research_step(&job_id).unwrap();
        assert!(!p1.terminado);
        assert_eq!(
            api.research_status(&job_id).unwrap().status,
            EstadoJob::Running
        );

        // Pausa a mitad de job: el estado persiste y el checkpoint del primer
        // stage ya quedó registrado.
        api.research_pause(&job_id).unwrap();
        assert_eq!(
            api.research_status(&job_id).unwrap().status,
            EstadoJob::Paused
        );
        let eventos_pausa = api.research_events(&job_id).unwrap();
        assert!(eventos_pausa.iter().any(|e| e.contains("job_paused")));
        let completados_antes: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM stages WHERE job_id = ?1 AND status = 'completed'",
                rusqlite::params![job_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            completados_antes, 1,
            "el primer stage quedó completo al pausar"
        );

        // Reanudar → el job vuelve a running y el step continúa desde donde
        // quedó (los stages completados antes de la pausa no se repiten).
        api.research_resume(&job_id).unwrap();
        assert_eq!(
            api.research_status(&job_id).unwrap().status,
            EstadoJob::Running
        );
        let mut pasos = 1;
        loop {
            let progreso = api.research_step(&job_id).unwrap();
            pasos += 1;
            if progreso.terminado {
                break;
            }
            assert!(pasos < 10);
        }
        // El DAG tiene 4 stages: 1 antes de la pausa + 3 después + el cierre
        // ocurre en el último paso.
        assert_eq!(pasos, 4, "sin pasos fantasma tras reanudar");
        let estado = api.research_status(&job_id).unwrap();
        assert_eq!(estado.status, EstadoJob::Done);
        let eventos = api.research_events(&job_id).unwrap();
        assert!(eventos.iter().any(|e| e.contains("job_paused")));
        assert!(eventos.iter().any(|e| e.contains("job_resumed")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mostrar_fuente_devuelve_el_asset_real() {
        let (db, repo, dir) = base();
        let llm = LlmSintetizaEvidencia;
        let api = api(&llm, &db, &repo, &dir);
        // asset-1 pertenece a item-1 en el corpus sintético.
        let assets = api.research_mostrar_fuente("item-1");
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].0, "escaneos/65-03-17-a.pdf");
        assert_eq!(assets[0].1, Some(3));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn los_jobs_reanudables_se_detectan() {
        let (db, repo, dir) = base();
        let llm = LlmSintetizaEvidencia;
        let api = api(&llm, &db, &repo, &dir);
        let job_id = api
            .research_start("pregunta", "proyecto", "{}", None, None)
            .unwrap();
        // Un paso y pausa: el job queda reanudable.
        let _ = api.research_step(&job_id).unwrap();
        api.research_pause(&job_id).unwrap();
        let reanudables = api.hay_jobs_reanudables();
        assert!(reanudables.contains(&job_id));
        // Cancelar lo saca de la lista.
        api.research_cancelar(&job_id).unwrap();
        let reanudables = api.hay_jobs_reanudables();
        assert!(!reanudables.contains(&job_id));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

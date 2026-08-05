//! Orquestador/planner (PLAN §6.2, §6.7, Fase 2).
//!
//! Núcleo = Orquestador → Workers → Verificador, con el plan como artefacto
//! persistido entre medio. El orquestador construye el plan (JSON, editable,
//! persistido en el job), arma el DAG de stages, ejecuta cada stage con un
//! worker acotado, verifica las afirmaciones, reformula consultas según el log
//! y decide el cierre. Su contexto está acotado a plan + resúmenes + log de
//! consultas — nunca chunks crudos (§6.1).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::cliente_llm::{ClienteLlm, TurnoAgente};
use crate::dominio::{ClaseFuente, Ledger, RelacionEvidencia, TipoClaim};
use crate::informe;
use crate::informe_secciones::{InformeSecciones, SeccionInforme};
use crate::memoria::{MemoriaDb, TipoMemoria};
use crate::prompts::PROMPT_ORQUESTADOR;
use crate::puerta_lectura::{self, Filtros, FuenteRecuperada};
use crate::recuperacion::Recuperador;
use crate::repositorio::{Cobertura, RepositorioSqlite};
use crate::trabajador::Worker;
use crate::trabajos::{ConfigJob, MotivoCierre, MotorTrabajos, Stage};
use crate::verificador::{EvidenciaConTexto, ModoVerificacion, Verificador};

/// Una etapa del plan (nodo del DAG de stages).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EtapaPlan {
    pub id: String,
    pub titulo: String,
    pub tipo: String,
    pub consultas: Vec<String>,
    pub depende_de: Vec<String>,
}

/// Plan de investigación persistido en el job (PLAN §6.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub hipotesis: String,
    pub etapas: Vec<EtapaPlan>,
    pub criterios_cierre: Vec<String>,
}

/// Genera el plan por defecto para un pedido (mecánico, sin LLM). El LLM puede
/// refinarlo (ver `refinar_plan`), pero este plan es la base determinista.
pub fn construir_plan(pedido: &str) -> Plan {
    Plan {
        hipotesis: pedido.to_string(),
        etapas: vec![
            EtapaPlan {
                id: "exploracion".into(),
                titulo: "Exploración y cobertura".into(),
                tipo: "exploracion".into(),
                consultas: vec![pedido.to_string()],
                depende_de: vec![],
            },
            EtapaPlan {
                id: "cronologia".into(),
                titulo: "Cronología".into(),
                tipo: "consulta".into(),
                consultas: vec![pedido.to_string()],
                depende_de: vec!["exploracion".into()],
            },
            EtapaPlan {
                id: "actores".into(),
                titulo: "Actores y organizaciones".into(),
                tipo: "consulta".into(),
                consultas: vec![format!("{pedido} actores")],
                depende_de: vec!["exploracion".into()],
            },
            EtapaPlan {
                id: "sintesis".into(),
                titulo: "Síntesis e informe".into(),
                tipo: "sintesis".into(),
                consultas: vec![],
                depende_de: vec!["cronologia".into(), "actores".into()],
            },
        ],
        criterios_cierre: vec![
            "todas las etapas completadas".to_string(),
            "budgets respetados".to_string(),
        ],
    }
}

/// Motor de investigación: ejecuta un job de punta a punta (Modo 1).
pub struct MotorInvestigacion<'a> {
    pub llm: &'a dyn ClienteLlm,
    pub repo: &'a RepositorioSqlite,
    pub recuperador: Option<&'a Recuperador>,
    pub motor: MotorTrabajos<'a>,
    pub ledger: Ledger<'a>,
    pub memoria: MemoriaDb<'a>,
    pub dir_artefactos: PathBuf,
    pub db: &'a crate::estado::EstadoDb,
}

impl<'a> MotorInvestigacion<'a> {
    /// Crea el job (con snapshot de reproducibilidad), arma el plan y ejecuta
    /// la investigación completa. Devuelve el id del job.
    pub fn crear_y_ejecutar(
        &self,
        pedido: &str,
        project: &str,
        config_snapshot: &str,
        max_cost: Option<f64>,
        max_llm_calls: Option<i64>,
    ) -> Result<String, String> {
        let corpus_snapshot = self.repo.snapshot_corpus()?;
        let job = self.motor.crear_job(ConfigJob {
            modo: "investigacion".into(),
            pregunta: pedido.into(),
            project: project.into(),
            corpus: "soip".into(),
            config_snapshot: config_snapshot.into(),
            corpus_snapshot_id: Some(corpus_snapshot),
            max_cost,
            max_llm_calls,
        })?;
        self.ejecutar_job(&job.id, pedido)?;
        Ok(job.id)
    }

    /// Ejecuta el plan de un job existente (resume incluido): prepara el plan,
    /// ejecuta todos los stages y cierra con el informe ensamblado.
    pub fn ejecutar_job(&self, job_id: &str, pedido: &str) -> Result<(), String> {
        self.preparar_job(job_id, pedido)?;
        while self.ejecutar_siguiente_stage(job_id)? {}
        self.cerrar_con_informe(job_id)?;
        Ok(())
    }

    /// Prepara el plan de un job: memoria longitudinal, plan persistido,
    /// stages del DAG y validación. Pensado para el step API (Fase 4).
    pub fn preparar_job(&self, job_id: &str, pedido: &str) -> Result<(), String> {
        let job = self
            .motor
            .obtener_job(job_id)
            .ok_or_else(|| format!("Job inexistente: {job_id}"))?;

        // Plan: base determinista, refinable por el LLM (si responde JSON).
        let plan = self
            .refinar_plan(pedido)
            .unwrap_or_else(|| construir_plan(pedido));
        let plan_json = serde_json::to_string(&plan).map_err(|e| format!("Plan ilegible: {e}"))?;
        self.motor.guardar_plan(job_id, &plan_json)?;
        self.motor.poner_en_marcha(job_id)?;

        // Stages del DAG según el plan.
        let mut ids_etapas: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for etapa in &plan.etapas {
            let stage_id = self
                .motor
                .agregar_stage(job_id, &etapa.tipo, &etapa.titulo)?;
            ids_etapas.insert(etapa.id.clone(), stage_id);
        }
        for etapa in &plan.etapas {
            for dep in &etapa.depende_de {
                if let (Some(stage), Some(dep_stage)) =
                    (ids_etapas.get(&etapa.id), ids_etapas.get(dep))
                {
                    self.motor.agregar_dependencia(stage, dep_stage)?;
                }
            }
        }
        if self.motor.validar_dag(job_id).is_err() {
            self.motor.cerrar(job_id, MotivoCierre::Blocked)?;
            return Err("El DAG del plan tiene un ciclo".into());
        }
        let _ = job;
        Ok(())
    }

    /// Ejecuta el siguiente stage del job (un worker por stage, con
    /// checkpoint). Devuelve `true` si quedan stages por ejecutar.
    pub fn ejecutar_siguiente_stage(&self, job_id: &str) -> Result<bool, String> {
        let Some(stage) = self.motor.siguiente_stage(job_id)? else {
            return Ok(false);
        };
        if self.motor.stage_reutilizable(job_id, &stage.id)? {
            self.motor.marcar_reutilizado(job_id, &stage.id)?;
            return Ok(true);
        }
        self.motor.marcar_inicio_stage(job_id, &stage.id)?;

        // Contexto de memoria longitudinal (retomar informes previos).
        let job = self
            .motor
            .obtener_job(job_id)
            .ok_or_else(|| format!("Job inexistente: {job_id}"))?;
        let previos = self.memoria.buscar(&job.project, &job.pregunta, 3);
        let memoria_ctx = if previos.is_empty() {
            String::new()
        } else {
            let resumen = previos
                .iter()
                .map(|m| format!("- {}: {}", m.title, m.content))
                .collect::<Vec<_>>()
                .join("\n");
            format!("\nHallazgos previos en esta línea de investigación:\n{resumen}")
        };

        // La etapa del plan correspondiente al stage.
        let plan: Option<Plan> = job
            .plan_json
            .as_deref()
            .and_then(|p| serde_json::from_str(p).ok());
        let etapa = plan.and_then(|p| {
            p.etapas
                .iter()
                .find(|e| {
                    self.motor
                        .stages(job_id)
                        .iter()
                        .any(|s| s.id == stage.id && s.titulo.as_deref() == Some(e.titulo.as_str()))
                })
                .cloned()
        });
        self.ejecutar_stage(job_id, &stage, &etapa, &memoria_ctx)?;
        Ok(true)
    }

    /// Cierra el job ensamblando el informe final con su cobertura declarada.
    pub fn cerrar_con_informe(&self, job_id: &str) -> Result<std::path::PathBuf, String> {
        let informe_path = self.ensamblar_informe(job_id)?;
        self.motor.cerrar(job_id, MotivoCierre::Completed)?;
        self.motor.registrar_evento(
            job_id,
            None,
            "informe_ensamblado",
            Some(&format!("{{\"path\":\"{}\"}}", informe_path.display())),
        )?;
        Ok(informe_path)
    }

    /// Refinamiento opcional del plan por el LLM. Si no responde un JSON
    /// válido, se usa el plan determinista.
    fn refinar_plan(&self, pedido: &str) -> Option<Plan> {
        let mensajes = vec![
            serde_json::json!({ "role": "system", "content": PROMPT_ORQUESTADOR }),
            serde_json::json!({
                "role": "user",
                "content": format!(
                    "Pedido: {pedido}\nDevolvé un plan JSON con el contrato: \
                     {{\"hipotesis\":\"…\",\"etapas\":[{{\"id\":\"…\",\"titulo\":\"…\",\
                     \"tipo\":\"consulta|sintesis|exploracion\",\"consultas\":[\"…\"],\
                     \"depende_de\":[\"…\"]}}],\"criterios_cierre\":[\"…\"]}}"
                )
            }),
        ];
        let turno = self.llm.turno_agente(&mensajes, &[]).ok()?;
        let texto = match turno {
            TurnoAgente::Texto(t) => t,
            TurnoAgente::Herramientas(_) => return None,
        };
        let inicio = texto.find('{')?;
        let fin = texto.rfind('}')?;
        serde_json::from_str(&texto[inicio..=fin]).ok()
    }

    /// Ejecuta un stage: recupera evidencia, la registra en el ledger, el
    /// worker sintetiza, las afirmaciones se verifican y la sección se guarda
    /// versionada.
    fn ejecutar_stage(
        &self,
        job_id: &str,
        stage: &Stage,
        etapa: &Option<EtapaPlan>,
        memoria_ctx: &str,
    ) -> Result<(), String> {
        let etapa = etapa.clone().unwrap_or(EtapaPlan {
            id: stage.id.clone(),
            titulo: stage.titulo.clone().unwrap_or_default(),
            tipo: stage.tipo.clone(),
            consultas: vec![],
            depende_de: vec![],
        });

        match etapa.tipo.as_str() {
            "exploracion" => {
                let cobertura = self.repo.cobertura();
                let seccion = SeccionInforme {
                    id: "exploracion".into(),
                    titulo: etapa.titulo.clone(),
                    contenido: informe::tabla_cobertura(&cobertura),
                    version: 1,
                    provenance: vec![],
                };
                let secciones = InformeSecciones::nuevo(self.db, &self.dir_artefactos);
                let ruta = secciones.guardar_seccion(job_id, &seccion)?;
                self.motor.completar_stage(
                    job_id,
                    &stage.id,
                    "sintesis",
                    &ruta.to_string_lossy(),
                )?;
                // Hallazgo de cobertura en la memoria longitudinal.
                self.memoria.guardar(
                    &self.proyecto(job_id),
                    "Cobertura del recorte",
                    TipoMemoria::Finding,
                    &format!(
                        "Cobertura: {} items totales, {} sin procesar.",
                        cobertura.items_total, cobertura.items_sin_procesar
                    ),
                    None,
                    None,
                )?;
            }
            "consulta" => {
                let mut todas: Vec<FuenteRecuperada> = Vec::new();
                let mut reformuladas: Vec<String> = Vec::new();
                for consulta in &etapa.consultas {
                    let filtros = Filtros::nuevo();
                    let pagina = puerta_lectura::buscar_con_filtros(self.repo, &filtros);
                    todas.extend(pagina.fuentes);
                    if let Some(rec) = self.recuperador {
                        let semanticas = rec.recuperar_consulta(self.repo, consulta, 8);
                        todas.extend(semanticas.into_iter().map(|f| FuenteRecuperada {
                            chunk_id: f.id,
                            item_id: String::new(),
                            asset_id: String::new(),
                            item_titulo: f.titulo,
                            coleccion: f.coleccion,
                            texto: f.contenido,
                            fecha: None,
                        }));
                    }
                    let q_id = self.motor.registrar_consulta(
                        job_id,
                        Some(&stage.id),
                        consulta,
                        None,
                        Some(pagina.total as i64),
                        reformuladas.last().map(|s| s.as_str()),
                    )?;
                    // Loop de reformulación (PLAN §6.2).
                    if let Some(variante) = reformular_consulta(consulta, pagina.total) {
                        let v_id = self.motor.registrar_consulta(
                            job_id,
                            Some(&stage.id),
                            &variante,
                            None,
                            None,
                            Some(&q_id),
                        )?;
                        reformuladas.push(v_id);
                    }
                }
                let unicas = puerta_lectura::deduplicar(todas);
                let evidencia = self.registrar_evidencia(job_id, unicas)?;
                let brief = format!(
                    "Stage: {}\nHipótesis: {}\nConsulta: {}\nObjetivo: aportar evidencia \
                     documental con su id para el informe.{memoria_ctx}",
                    etapa.titulo,
                    etapa.consultas.join("; "),
                    etapa.consultas.first().cloned().unwrap_or_default(),
                );
                let worker = Worker::nuevo(self.llm);
                let sintesis = worker.sintetizar(&stage.id, &brief, &evidencia)?;

                let ruta = self
                    .dir_artefactos
                    .join(job_id)
                    .join("sintesis")
                    .join(format!("{}.md", stage.id));
                std::fs::create_dir_all(ruta.parent().unwrap())
                    .map_err(|e| format!("No se pudo crear sintesis/: {e}"))?;
                std::fs::write(&ruta, &sintesis.texto)
                    .map_err(|e| format!("No se pudo escribir la síntesis: {e}"))?;

                // Verificación de las afirmaciones del worker (PLAN §6.1).
                self.verificar_claims(job_id, &sintesis.claims, &evidencia)?;

                // Sección versionada del informe (id = id de la etapa del plan).
                let seccion = SeccionInforme {
                    id: etapa.id.clone(),
                    titulo: etapa.titulo.clone(),
                    contenido: sintesis.texto.clone(),
                    version: 1,
                    provenance: sintesis
                        .claims
                        .iter()
                        .flat_map(|c| c.evidencia_ids.clone())
                        .collect(),
                };
                let secciones = InformeSecciones::nuevo(self.db, &self.dir_artefactos);
                let ruta_seccion = secciones.guardar_seccion(job_id, &seccion)?;

                self.motor.completar_stage(
                    job_id,
                    &stage.id,
                    "sintesis",
                    &ruta_seccion.to_string_lossy(),
                )?;

                // Hallazgo en la memoria longitudinal, ligado a su evidencia.
                let primer_claim = sintesis.claims.first().cloned();
                if let Some(claim) = primer_claim {
                    let (mem_id, _) = self.memoria.guardar(
                        &self.proyecto(job_id),
                        &etapa.titulo,
                        TipoMemoria::Finding,
                        &claim.texto,
                        None,
                        None,
                    )?;
                    for eid in claim.evidencia_ids {
                        let _ = self
                            .ledger
                            .ligar_memoria_evidencia(&mem_id, &eid, "supports");
                    }
                }
            }
            "sintesis" => {
                // Brief = secciones de los stages previos (síntesis jerárquica).
                let secciones = InformeSecciones::nuevo(self.db, &self.dir_artefactos);
                let previas: Vec<String> = secciones
                    .secciones_del_job(job_id)
                    .into_iter()
                    .filter(|s| s != "sintesis")
                    .map(|id| {
                        secciones
                            .leer_seccion(job_id, &id)
                            .map(|s| format!("## {}\n{}", s.titulo, s.contenido))
                            .unwrap_or_default()
                    })
                    .collect();
                let brief = format!(
                    "Stage: {}\nIntegrá las secciones previas en la síntesis final \
                     conservando las referencias de evidencia.{memoria_ctx}\n\n{}",
                    etapa.titulo,
                    previas.join("\n\n")
                );
                let worker = Worker::nuevo(self.llm);
                let sintesis = worker.sintetizar(&stage.id, &brief, &[])?;
                let seccion = SeccionInforme {
                    id: "sintesis".into(),
                    titulo: etapa.titulo.clone(),
                    contenido: sintesis.texto.clone(),
                    version: 1,
                    provenance: vec![],
                };
                let ruta = secciones.guardar_seccion(job_id, &seccion)?;
                self.motor.completar_stage(
                    job_id,
                    &stage.id,
                    "informe_parcial",
                    &ruta.to_string_lossy(),
                )?;
            }
            otro => {
                return Err(format!("Tipo de etapa desconocido: {otro}"));
            }
        }
        Ok(())
    }

    /// Registra las fuentes (clase 1) y las evidencias del stage en el ledger.
    fn registrar_evidencia(
        &self,
        job_id: &str,
        fuentes: Vec<FuenteRecuperada>,
    ) -> Result<Vec<EvidenciaConTexto>, String> {
        let proyecto = self.proyecto(job_id);
        let mut out = Vec::new();
        for f in fuentes.into_iter().take(8) {
            let src = self.ledger.registrar_fuente(
                ClaseFuente::EntropiaChunk,
                Some(&f.chunk_id),
                if f.item_id.is_empty() {
                    None
                } else {
                    Some(&f.item_id)
                },
                if f.asset_id.is_empty() {
                    None
                } else {
                    Some(&f.asset_id)
                },
                Some(&f.chunk_id),
                None,
                &proyecto,
                "soip",
            )?;
            let fin = f.texto.chars().count() as i64;
            let ev = self
                .ledger
                .registrar_evidencia(&src, &f.texto, 0, fin, None, Some(0.9))?;
            // document_date por capas de fechas.rs → source_temporal_metadata
            // (vive en estado.sqlite, nunca en las tablas de Lite/Pro).
            if let Some(cand) = crate::fechas::fecha_desde_titulo(&f.item_titulo, &f.coleccion) {
                if let Some(fecha) = cand.fecha {
                    let _ = self.ledger.registrar_metadata_temporal(
                        &src,
                        &fecha.iso(),
                        cand.precision.as_str(),
                        cand.confidence,
                        &cand.source,
                    );
                }
            }
            out.push(EvidenciaConTexto {
                id: ev,
                quote: f.texto.clone(),
                span_start: 0,
                span_end: fin,
                texto_fuente: f.texto,
                relacion: "supports".into(),
            });
        }
        Ok(out)
    }

    /// Verifica cada claim del worker y registra el run en `verification_runs`.
    fn verificar_claims(
        &self,
        job_id: &str,
        claims: &[crate::trabajador::ClaimPropuesta],
        evidencia: &[EvidenciaConTexto],
    ) -> Result<(), String> {
        let por_id: std::collections::HashMap<&str, &EvidenciaConTexto> =
            evidencia.iter().map(|e| (e.id.as_str(), e)).collect();
        let verificador = Verificador::nuevo(self.llm);
        let cobertura: Cobertura = self.repo.cobertura();
        for claim in claims {
            let claim_id = self
                .ledger
                .registrar_claim(job_id, claim.tipo, &claim.texto)?;
            let mut lote: Vec<EvidenciaConTexto> = Vec::new();
            for eid in &claim.evidencia_ids {
                if let Some(e) = por_id.get(eid.as_str()) {
                    lote.push((*e).clone());
                    let relation = if e.relacion == "contradicts" {
                        RelacionEvidencia::Contradicts
                    } else {
                        RelacionEvidencia::Supports
                    };
                    self.ledger
                        .relacionar(&claim_id, eid, relation, Some(0.9))?;
                }
            }
            // El Verifier decide con el protocolo aislado; el LLM de la
            // verificación es un rol distinto del productor (mismo modelo,
            // otro prompt y contexto restringido).
            let resultado = verificador.verificar(
                &claim.texto,
                &lote,
                if claim.tipo == TipoClaim::Interpretive {
                    ModoVerificacion::Interpretativo
                } else {
                    ModoVerificacion::Factual
                },
                Some(&cobertura),
            )?;
            self.ledger.registrar_verificacion(
                &claim_id,
                resultado.estado,
                Some(self.llm.modelo()),
                Some(&resultado.prompt_hash),
                Some(&resultado.evidencia_considerada),
                if resultado.contraevidencia.is_empty() {
                    None
                } else {
                    Some(&resultado.contraevidencia)
                },
                Some(&resultado.rationale),
                resultado.error_kind.as_deref(),
                true,
            )?;
        }
        Ok(())
    }

    /// Ensambla el informe final: tabla de cobertura + secciones versionadas.
    pub fn ensamblar_informe(&self, job_id: &str) -> Result<PathBuf, String> {
        let secciones = InformeSecciones::nuevo(self.db, &self.dir_artefactos);
        let ids = secciones.secciones_del_job(job_id);
        let mut cuerpo = String::new();
        for id in ids {
            if let Some(s) = secciones.leer_seccion(job_id, &id) {
                cuerpo.push_str(&format!("\n\n{}", s.contenido));
            }
        }
        let cobertura = self.repo.cobertura();
        let titulo = self
            .motor
            .obtener_job(job_id)
            .map(|j| j.pregunta)
            .unwrap_or_else(|| "informe".to_string());
        let dir_informes = self.dir_artefactos.join("informes");
        let completo = format!("{}{}", informe::tabla_cobertura(&cobertura), cuerpo.trim());
        let ruta = informe::guardar(&dir_informes, &titulo, &completo)?;
        Ok(ruta)
    }

    fn proyecto(&self, job_id: &str) -> String {
        self.motor
            .obtener_job(job_id)
            .map(|j| j.project)
            .unwrap_or_else(|| "sin-proyecto".into())
    }
}

/// Reformulación determinista de consultas según el recubrimiento (PLAN §6.2):
/// muy pocos resultados → variante más amplia; demasiados → acota por período.
pub fn reformular_consulta(consulta: &str, total: usize) -> Option<String> {
    match total {
        0..=2 => {
            // Ampliar: quita el año (si lo hay) y suma sinónimos amplios.
            let sin_anio = consulta
                .split_whitespace()
                .filter(|t| !(t.len() == 4 && t.chars().all(|c| c.is_ascii_digit())))
                .collect::<Vec<_>>()
                .join(" ");
            Some(format!(
                "{sin_anio} conflicto OR huelga OR paro OR asamblea"
            ))
        }
        3..=40 => None,
        _ => Some(format!("{consulta} 1965")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::estado::EstadoDb;
    use crate::llm_fake::LlmSintetizaEvidencia;
    use crate::memoria::MemoriaDb;
    use crate::repositorio::RepositorioSqlite;
    use crate::trabajos::EstadoJob;

    fn base_e2e() -> (EstadoDb, RepositorioSqlite) {
        let repo =
            RepositorioSqlite::abrir(crate::tests_comunes::corpus_sintetico().to_str().unwrap())
                .unwrap();
        let db = EstadoDb::abrir_en_memoria().unwrap();
        (db, repo)
    }

    #[test]
    fn una_investigacion_multietapa_ejecuta_y_deja_trazabilidad() {
        let (db, repo) = base_e2e();
        let dir = std::env::temp_dir().join(format!(
            "entropia-e2e-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let llm = LlmSintetizaEvidencia;
        let motor = MotorInvestigacion {
            llm: &llm,
            repo: &repo,
            recuperador: None,
            motor: MotorTrabajos::nuevo(&db),
            ledger: Ledger::nuevo(&db),
            memoria: MemoriaDb::nuevo(&db),
            dir_artefactos: dir.clone(),
            db: &db,
        };

        let job_id = motor
            .crear_y_ejecutar(
                "¿Conflictividad obrera en el SOIP?",
                "soip-conflictividad",
                "{\"modelo\":\"fake/sintetiza\",\"denylist\":[]}",
                None,
                None,
            )
            .unwrap();

        // 1. El job cerró con motivo.
        let job = motor.motor.obtener_job(&job_id).unwrap();
        assert_eq!(job.status, EstadoJob::Done);
        assert_eq!(job.close_reason, Some(MotivoCierre::Completed));

        // 2. El plan quedó persistido en el job.
        let plan_json = job.plan_json.clone().unwrap();
        let plan: Plan = serde_json::from_str(&plan_json).unwrap();
        assert!(!plan.etapas.is_empty());

        // 3. Queries en el log (memoria de búsqueda): una por etapa de
        // consulta (cronología y actores).
        let consultas: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM queries WHERE job_id = ?1",
                rusqlite::params![job_id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            consultas >= 2,
            "deben quedar consultas en el log, había {consultas}"
        );

        // 4. Trazabilidad: sources y evidence registradas.
        let sources: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sources WHERE project = 'soip-conflictividad'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(sources >= 1);

        // 5. Claims verificados con su run.
        let runs: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM verification_runs vr \
                 JOIN claims c ON c.id = vr.claim_id WHERE c.job_id = ?1 AND vr.aceptado = 1",
                rusqlite::params![job_id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            runs >= 1,
            "debe haber al menos un run de verificación aceptado"
        );
        let estados: Vec<String> = db
            .conn()
            .prepare(
                "SELECT vr.estado FROM verification_runs vr \
                 JOIN claims c ON c.id = vr.claim_id WHERE c.job_id = ?1",
            )
            .unwrap()
            .query_map(rusqlite::params![job_id], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            estados.iter().any(|e| e == "supported"),
            "el claim igual a su cita debe quedar supported, estados: {estados:?}"
        );

        // 6. Secciones versionadas (exploracion, cronologia, actores, sintesis).
        let secciones = InformeSecciones::nuevo(&db, &dir);
        let ids = secciones.secciones_del_job(&job_id);
        assert!(ids.contains(&"exploracion".to_string()));
        assert!(ids.contains(&"cronologia".to_string()));
        assert!(ids.contains(&"sintesis".to_string()));

        // 7. Memoria longitudinal guardada.
        let hallazgos = MemoriaDb::nuevo(&db).buscar("soip-conflictividad", "huelga", 5);
        assert!(!hallazgos.is_empty());

        // 8. Informe ensamblado con cobertura declarada.
        let informe_path = motor.ensamblar_informe(&job_id).unwrap();
        let contenido = std::fs::read_to_string(&informe_path).unwrap();
        assert!(contenido.contains("Cobertura del recorte consultado"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn el_plan_determinista_tiene_etapas_y_dependencias() {
        let plan = construir_plan("conflictividad por décadas");
        assert_eq!(plan.etapas.len(), 4);
        let sintesis = plan.etapas.iter().find(|e| e.id == "sintesis").unwrap();
        assert!(sintesis.depende_de.contains(&"cronologia".to_string()));
        assert!(sintesis.depende_de.contains(&"actores".to_string()));
        assert_eq!(plan.hipotesis, "conflictividad por décadas");
    }

    #[test]
    fn la_memoria_longitudinal_se_consulta_al_retomar_un_informe() {
        let (db, repo) = base_e2e();
        let dir = std::env::temp_dir().join(format!("entropia-e2e-mem-{}", std::process::id()));
        // Un hallazgo previo de la misma línea de investigación.
        MemoriaDb::nuevo(&db)
            .guardar(
                "soip-conflictividad",
                "Hallazgo previo",
                TipoMemoria::Finding,
                "Conflictividad obrera: la huelga comenzó en marzo de 1965.",
                None,
                None,
            )
            .unwrap();

        let llm = crate::llm_fake::LlmGrabador::nuevo("síntesis sin afirmaciones");
        let motor = MotorInvestigacion {
            llm: &llm,
            repo: &repo,
            recuperador: None,
            motor: MotorTrabajos::nuevo(&db),
            ledger: Ledger::nuevo(&db),
            memoria: MemoriaDb::nuevo(&db),
            dir_artefactos: dir.clone(),
            db: &db,
        };
        let job_id = motor
            .crear_y_ejecutar(
                "¿Conflictividad obrera en el SOIP?",
                "soip-conflictividad",
                "{}",
                None,
                None,
            )
            .unwrap();
        let job = motor.motor.obtener_job(&job_id).unwrap();
        assert_eq!(job.status, EstadoJob::Done);

        // El brief de los workers incluyó los hallazgos previos (resume de un
        // informe anterior consultando la memoria longitudinal).
        let registro = llm.registro.lock().unwrap();
        let todo = registro.join("\n");
        assert!(
            todo.contains("Hallazgos previos en esta línea de investigación"),
            "el contexto de memoria debe llegar al worker: {todo}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn la_reformulacion_depende_del_recubrimiento() {
        assert!(reformular_consulta("huelga", 1).is_some());
        assert!(reformular_consulta("huelga", 20).is_none());
        assert!(reformular_consulta("huelga", 80).is_some());
        // Ampliar quita el año (si lo hay) para ampliar la cobertura.
        let ampliada = reformular_consulta("huelga 1965", 1).unwrap();
        assert!(!ampliada.contains("1965"));
    }
}

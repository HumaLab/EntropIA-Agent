//! Modo 2 — Redactor de Papers (PLAN §3, Fase 3).
//!
//! Producción asistida de artículos combinando: informes previos del agente
//! (clase 2), fuentes primarias de EntropIA (clase 1), bibliografía de Zotero
//! (clase 3) y bibliografía web (clase 4). Separación estricta de provenance:
//! cada afirmación queda etiquetada por la clase de sus evidencias (vía
//! ClaimEvidence → evidence → sources.kind). Una referencia sin texto
//! accesible deja la evidencia `unverifiable` y se eleva al historiador.

use std::path::PathBuf;

use crate::cliente_llm::ClienteLlm;
use crate::dominio::{ClaseFuente, EstadoEpistemico, Ledger, RelacionEvidencia};
use crate::especialistas::bibliografia_web::Obra;
use crate::especialistas::zotero::ItemZotero;
use crate::estado::EstadoDb;
use crate::informe_secciones::{InformeSecciones, SeccionInforme};
use crate::prompts::PROMPT_REDACTOR;
use crate::trabajador::{ClaimPropuesta, Worker};
use crate::trabajos::{ConfigJob, MotivoCierre, MotorTrabajos};
use crate::verificador::{EvidenciaConTexto, ModoVerificacion, Verificador};

/// Una fuente bibliográfica del paper (clase 3 o 4).
#[derive(Debug, Clone)]
pub struct FuenteBibliografica {
    pub kind: ClaseFuente,
    pub external_id: String,
    pub titulo: String,
    pub autores: Vec<String>,
    pub anio: Option<i64>,
    pub doi: Option<String>,
    pub texto_accesible: bool,
}

/// Convierte los items de Zotero en fuentes bibliográficas (clase 3).
pub fn desde_zotero(items: &[ItemZotero]) -> Vec<FuenteBibliografica> {
    items
        .iter()
        .map(|i| FuenteBibliografica {
            kind: ClaseFuente::Zotero,
            external_id: i.key.clone(),
            titulo: i.title.clone(),
            autores: i.creators.clone(),
            anio: i
                .date
                .as_deref()
                .and_then(|d| d.chars().take(4).collect::<String>().parse().ok()),
            doi: i.doi.clone(),
            texto_accesible: true,
        })
        .collect()
}

/// Convierte las obras de OpenAlex/CrossRef en fuentes bibliográficas
/// (clase 4). Sin texto accesible → la evidencia queda unverifiable.
pub fn desde_web(obras: &[Obra]) -> Vec<FuenteBibliografica> {
    obras
        .iter()
        .map(|o| FuenteBibliografica {
            kind: ClaseFuente::External,
            external_id: o
                .doi
                .clone()
                .unwrap_or_else(|| format!("titulo:{}", o.titulo)),
            titulo: o.titulo.clone(),
            autores: o.autores.clone(),
            anio: o.anio,
            doi: o.doi.clone(),
            texto_accesible: crate::especialistas::bibliografia_web::texto_accesible(o),
        })
        .collect()
}

/// Un informe previo del agente (clase 2) para citar en el paper.
#[derive(Debug, Clone)]
pub struct InformePrevIo {
    pub id: String,
    pub titulo: String,
    pub texto: String,
}

/// Redactor de papers sobre el ledger.
pub struct RedactorPaper<'a> {
    pub llm: &'a dyn ClienteLlm,
    pub ledger: Ledger<'a>,
    pub db: &'a EstadoDb,
    pub dir_paper: PathBuf,
}

impl<'a> RedactorPaper<'a> {
    /// Redacta el paper completo y devuelve el id del job de tipo `paper`.
    ///
    /// Workflow (PLAN §9, Fase 3): identificar bibliografía → relacionar con
    /// resultados sobre fuentes primarias → estado de la cuestión → contraste
    /// de interpretaciones → redacción por secciones.
    #[allow(clippy::too_many_arguments)]
    pub fn redactar(
        &self,
        tema: &str,
        project: &str,
        zotero: &[ItemZotero],
        web: &[Obra],
        informes_previos: &[InformePrevIo],
        evidencias_primarias: &[EvidenciaConTexto],
    ) -> Result<String, String> {
        // 1. Job del paper.
        let motor = MotorTrabajos::nuevo(self.db);
        let job = motor.crear_job(ConfigJob {
            modo: "paper".into(),
            pregunta: tema.into(),
            project: project.into(),
            corpus: "soip".into(),
            config_snapshot: "{\"modo\":\"paper\"}".into(),
            corpus_snapshot_id: None,
            max_cost: None,
            max_llm_calls: None,
        })?;
        let paper_id = job.id;

        // 2. Identificar bibliografía (clases 3 y 4).
        let bibliografia: Vec<FuenteBibliografica> = desde_zotero(zotero)
            .into_iter()
            .chain(desde_web(web))
            .collect();
        let (ev_biblio, _sin_texto) = self.registrar_bibliografia(&paper_id, &bibliografia)?;

        // 3. Informes previos (clase 2).
        let ev_previos = self.registrar_informes_previos(&paper_id, informes_previos)?;

        // 4. Redacción por secciones (cada una con su bloque de evidencia).
        let secciones = InformeSecciones::nuevo(self.db, &self.dir_paper);
        let worker = Worker::nuevo(self.llm);
        let verificador = Verificador::nuevo(self.llm);

        let secciones_plan = [
            (
                "estado_de_la_cuestion",
                "Estado de la cuestión",
                format!("Bibliografía sobre: {tema}"),
                ev_biblio.clone(),
            ),
            (
                "resultados",
                "Resultados sobre fuentes primarias",
                format!(
                    "Resultados del agente sobre fuentes primarias y sus informes previos: {tema}"
                ),
                {
                    let mut v = ev_previos.clone();
                    v.extend(evidencias_primarias.iter().cloned());
                    v
                },
            ),
            (
                "contraste",
                "Contraste de interpretaciones",
                format!("Interpretaciones rivales sobre: {tema}"),
                {
                    let mut v = ev_biblio.clone();
                    v.extend(evidencias_primarias.iter().cloned());
                    v
                },
            ),
            (
                "conclusiones",
                "Conclusiones",
                format!("Conclusiones del paper: {tema}"),
                {
                    let mut v = ev_biblio.clone();
                    v.extend(ev_previos.clone());
                    v.extend(evidencias_primarias.iter().cloned());
                    v
                },
            ),
        ];

        for (id, titulo, brief, evidencias) in secciones_plan {
            let sintesis =
                worker.sintetizar_con_prompt(PROMPT_REDACTOR, id, &brief, &evidencias)?;
            // Claims con verificación y puerta de texto accesible.
            let mut claims_ok: Vec<ClaimPropuesta> = Vec::new();
            for claim in &sintesis.claims {
                let claim_id = self
                    .ledger
                    .registrar_claim(&paper_id, claim.tipo, &claim.texto)?;
                let mut lote: Vec<EvidenciaConTexto> = Vec::new();
                let mut solo_sin_texto = !evidencias.is_empty();
                for eid in &claim.evidencia_ids {
                    if let Some(e) = evidencias.iter().find(|e| &e.id == eid) {
                        lote.push(e.clone());
                        self.ledger.relacionar(
                            &claim_id,
                            eid,
                            RelacionEvidencia::Supports,
                            Some(0.9),
                        )?;
                        // ¿La fuente de esta evidencia tiene texto accesible?
                        if self.evidencia_con_texto(e) {
                            solo_sin_texto = false;
                        }
                    }
                }
                let mut resultado =
                    verificador.verificar(&claim.texto, &lote, ModoVerificacion::Factual, None)?;
                // Puerta de acceso (PLAN §11#8): sin texto accesible, la
                // evidencia queda unverifiable y se eleva al historiador.
                if solo_sin_texto && !lote.is_empty() {
                    resultado = crate::verificador::ResultadoVerificacion {
                        estado: EstadoEpistemico::Unverifiable,
                        rationale: "La referencia no tiene texto accesible: la evidencia queda \
                                    unverifiable y se eleva al historiador."
                            .into(),
                        error_kind: Some("knowledge_lack".into()),
                        contraevidencia: String::new(),
                        evidencia_considerada: resultado.evidencia_considerada,
                        prompt_hash: resultado.prompt_hash,
                    };
                }
                self.ledger.registrar_verificacion(
                    &claim_id,
                    resultado.estado,
                    Some(self.llm.modelo()),
                    Some(&resultado.prompt_hash),
                    Some(&resultado.evidencia_considerada),
                    None,
                    Some(&resultado.rationale),
                    resultado.error_kind.as_deref(),
                    true,
                )?;
                claims_ok.push(claim.clone());
            }
            // Sección versionada como artefacto.
            let seccion = SeccionInforme {
                id: id.into(),
                titulo: titulo.into(),
                contenido: sintesis.texto.clone(),
                version: 1,
                provenance: claims_ok
                    .iter()
                    .flat_map(|c| c.evidencia_ids.clone())
                    .collect(),
            };
            secciones.guardar_seccion(&paper_id, &seccion)?;
            motor.registrar_evento(
                &paper_id,
                Some(id),
                "seccion_paper",
                Some(&format!("{{\"seccion\":\"{id}\"}}")),
            )?;
        }

        motor.cerrar(&paper_id, MotivoCierre::Completed)?;
        Ok(paper_id)
    }

    /// Registra la bibliografía (clases 3 y 4) y devuelve sus evidencias y las
    /// fuentes sin texto accesible.
    fn registrar_bibliografia(
        &self,
        paper_id: &str,
        bibliografia: &[FuenteBibliografica],
    ) -> Result<(Vec<EvidenciaConTexto>, Vec<String>), String> {
        let mut evidencias = Vec::new();
        let mut sin_texto = Vec::new();
        for b in bibliografia {
            let src = self.ledger.registrar_fuente(
                b.kind,
                Some(&b.external_id),
                None,
                None,
                None,
                Some(&format!("bibliografia:{}", b.titulo)),
                self.proyecto(paper_id).as_str(),
                "soip",
            )?;
            let metadata = serde_json::json!({
                "titulo": b.titulo,
                "autores": b.autores,
                "anio": b.anio,
                "doi": b.doi,
                "kind": b.kind.as_str(),
            })
            .to_string();
            let excerpt = if b.texto_accesible {
                Some(format!(
                    "{} — {} ({})",
                    b.titulo,
                    b.autores.join("; "),
                    b.anio.map(|a| a.to_string()).unwrap_or_default()
                ))
            } else {
                sin_texto.push(src.clone());
                None
            };
            self.ledger
                .registrar_version_fuente(&src, &metadata, excerpt.as_deref())?;
            // Evidencia = la metadata bibliográfica (clase 3/4).
            let texto_ev = metadata.clone();
            let fin = texto_ev.chars().count() as i64;
            let ev = self
                .ledger
                .registrar_evidencia(&src, &texto_ev, 0, fin, None, Some(1.0))?;
            evidencias.push(EvidenciaConTexto {
                id: ev,
                quote: texto_ev.clone(),
                span_start: 0,
                span_end: fin,
                texto_fuente: texto_ev,
                relacion: "supports".into(),
            });
        }
        Ok((evidencias, sin_texto))
    }

    /// Registra los informes previos (clase 2) como fuentes con texto.
    fn registrar_informes_previos(
        &self,
        paper_id: &str,
        informes: &[InformePrevIo],
    ) -> Result<Vec<EvidenciaConTexto>, String> {
        let mut evidencias = Vec::new();
        for informe in informes {
            let src = self.ledger.registrar_fuente(
                ClaseFuente::AgentReport,
                Some(&informe.id),
                None,
                None,
                None,
                None,
                self.proyecto(paper_id).as_str(),
                "soip",
            )?;
            self.ledger.registrar_version_fuente(
                &src,
                &format!("{{\"titulo\":\"{}\"}}", informe.titulo),
                Some(&informe.texto),
            )?;
            let fin = informe.texto.chars().count() as i64;
            let ev =
                self.ledger
                    .registrar_evidencia(&src, &informe.texto, 0, fin, None, Some(0.95))?;
            evidencias.push(EvidenciaConTexto {
                id: ev,
                quote: informe.texto.clone(),
                span_start: 0,
                span_end: fin,
                texto_fuente: informe.texto.clone(),
                relacion: "supports".into(),
            });
        }
        Ok(evidencias)
    }

    /// ¿La evidencia tiene texto accesible en su fuente?
    fn evidencia_con_texto(&self, e: &EvidenciaConTexto) -> bool {
        self.ledger
            .evidencia(&e.id)
            .and_then(|ev| self.ledger.fuente(&ev.source_id))
            .map(|f| {
                if f.kind == "entropia_chunk" {
                    true // clase 1: el texto del chunk es accesible por definición
                } else {
                    self.ledger.fuente_tiene_texto(&f.id)
                }
            })
            .unwrap_or(false)
    }

    /// Clases de provenance de las afirmaciones de un paper.
    pub fn clases_de_las_afirmaciones(&self, paper_id: &str) -> Vec<(String, Vec<u8>)> {
        let claims: Vec<crate::dominio::Claim> =
            {
                let Ok(mut stmt) = self.db.conn().prepare(
                    "SELECT id, job_id, type, texto, status FROM claims WHERE job_id = ?1",
                ) else {
                    return Vec::new();
                };
                let Ok(rows) = stmt.query_map(rusqlite::params![paper_id], |r| {
                    Ok(crate::dominio::Claim {
                        id: r.get(0)?,
                        job_id: r.get(1)?,
                        tipo: r.get(2)?,
                        texto: r.get(3)?,
                        status: r.get(4)?,
                    })
                }) else {
                    return Vec::new();
                };
                rows.filter_map(|r| r.ok()).collect()
            };
        claims
            .into_iter()
            .map(|c| {
                let mut clases = self.ledger.clases_del_claim(&c.id);
                clases.sort_unstable();
                (c.id, clases)
            })
            .collect()
    }

    fn proyecto(&self, job_id: &str) -> String {
        MotorTrabajos::nuevo(self.db)
            .obtener_job(job_id)
            .map(|j| j.project)
            .unwrap_or_else(|| "sin-proyecto".into())
    }
}

/// Lee los informes previos del agente desde los artefactos en disco
/// (`dir/{job_id}/secciones/*.md`), para citarlos como clase 2.
pub fn leer_informes_previos(dir: &std::path::Path, jobs: &[String]) -> Vec<InformePrevIo> {
    let mut out = Vec::new();
    for job in jobs {
        let secciones_dir = dir.join(job).join("secciones");
        let Ok(entries) = std::fs::read_dir(&secciones_dir) else {
            continue;
        };
        let mut texto = String::new();
        for entry in entries.flatten() {
            if let Ok(contenido) = std::fs::read_to_string(entry.path()) {
                texto.push_str(&contenido);
                texto.push('\n');
            }
        }
        if !texto.is_empty() {
            out.push(InformePrevIo {
                id: job.clone(),
                titulo: format!("informe {job}"),
                texto,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::especialistas::bibliografia_web::Obra;
    use crate::especialistas::zotero::ItemZotero;
    use crate::estado::EstadoDb;
    use crate::llm_fake::LlmSintetizaEvidencia;
    use crate::memoria::MemoriaDb;
    use crate::trabajos::EstadoJob;
    use crate::verificador::EvidenciaConTexto;

    fn item_zotero(key: &str, titulo: &str) -> ItemZotero {
        ItemZotero {
            key: key.into(),
            item_type: "journalArticle".into(),
            title: titulo.into(),
            creators: vec!["Ana González".into()],
            date: Some("1985".into()),
            doi: Some("10.1000/z".into()),
        }
    }

    fn obra(doi: &str, titulo: &str, oa: bool) -> Obra {
        Obra {
            titulo: titulo.into(),
            autores: vec!["María López".into()],
            anio: Some(2020),
            doi: Some(doi.into()),
            fuente: "openalex".into(),
            open_access: oa,
        }
    }

    /// Registra la evidencia primaria en el ledger y devuelve su versión real
    /// (con id persistido) para pasar al paper.
    fn evidencia_primaria_registrada(ledger: &Ledger) -> EvidenciaConTexto {
        let src = ledger
            .registrar_fuente(
                ClaseFuente::EntropiaChunk,
                Some("chunk-1"),
                None,
                None,
                Some("chunk-1"),
                None,
                "soip-conflictividad",
                "soip",
            )
            .unwrap();
        let texto = "La huelga comenzó el 17 de marzo de 1965.";
        let fin = texto.chars().count() as i64;
        let ev = ledger
            .registrar_evidencia(&src, texto, 0, fin, None, Some(0.95))
            .unwrap();
        EvidenciaConTexto {
            id: ev,
            quote: texto.into(),
            span_start: 0,
            span_end: fin,
            texto_fuente: texto.into(),
            relacion: "supports".into(),
        }
    }

    #[test]
    fn el_paper_combina_las_cuatro_clases_y_etiqueta_cada_afirmacion() {
        let db = EstadoDb::abrir_en_memoria().unwrap();
        let dir = std::env::temp_dir().join(format!("entropia-paper-test-{}", std::process::id()));
        let llm = LlmSintetizaEvidencia;
        let redactor = RedactorPaper {
            llm: &llm,
            ledger: Ledger::nuevo(&db),
            db: &db,
            dir_paper: dir.clone(),
        };

        // El fake emite una afirmación por evidencia de cada bloque de sección.
        let zotero = vec![item_zotero("Z1", "La pesca en Mar del Plata")];
        let web = vec![
            obra("10.1000/oa", "Conflictividad obrera (OA)", true),
            obra("10.1000/no", "Texto cerrado (sin acceso)", false),
        ];
        let previo = InformePrevIo {
            id: "informe-1".into(),
            titulo: "Informe previo".into(),
            texto: "La conflictividad aumentó en 1965.".into(),
        };
        let primaria = evidencia_primaria_registrada(&redactor.ledger);
        let paper_id = redactor
            .redactar(
                "La conflictividad obrera",
                "soip-conflictividad",
                &zotero,
                &web,
                &[previo],
                &[primaria],
            )
            .unwrap();

        // Job cerrado como paper.
        let job = MotorTrabajos::nuevo(&db).obtener_job(&paper_id).unwrap();
        assert_eq!(job.modo, "paper");
        assert_eq!(job.status, EstadoJob::Done);

        // Cada afirmación queda etiquetada con la clase de sus evidencias.
        let etiquetadas = redactor.clases_de_las_afirmaciones(&paper_id);
        // El fake emite una afirmación por evidencia de cada sección.
        assert!(
            etiquetadas.len() >= 8,
            "debe haber una afirmación por evidencia: {}",
            etiquetadas.len()
        );
        for (claim_id, clases) in &etiquetadas {
            assert_eq!(
                clases.len(),
                1,
                "cada afirmación tiene una clase {claim_id}: {clases:?}"
            );
        }
        // El paper combina las cuatro clases de provenance.
        let clases: Vec<u8> = etiquetadas.iter().map(|(_, c)| c[0]).collect();
        assert!(clases.contains(&1));
        assert!(clases.contains(&2));
        assert!(clases.contains(&3));
        assert!(clases.contains(&4));

        // Secciones guardadas como artefactos.
        let secciones = InformeSecciones::nuevo(&db, &dir);
        let ids = secciones.secciones_del_job(&paper_id);
        assert!(ids.contains(&"estado_de_la_cuestion".to_string()));
        assert!(ids.contains(&"conclusiones".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn una_referencia_sin_texto_accesible_queda_unverifiable() {
        let db = EstadoDb::abrir_en_memoria().unwrap();
        let dir =
            std::env::temp_dir().join(format!("entropia-paper-sinacceso-{}", std::process::id()));
        let llm = LlmSintetizaEvidencia;
        let redactor = RedactorPaper {
            llm: &llm,
            ledger: Ledger::nuevo(&db),
            db: &db,
            dir_paper: dir.clone(),
        };

        // Solo bibliografía sin texto accesible: la primera evidencia de
        // «estado_de_la_cuestion» es la obra cerrada.
        let web = vec![obra("10.1000/no", "Texto cerrado (sin acceso)", false)];
        let paper_id = redactor
            .redactar("Un tema", "proyecto", &[], &web, &[], &[])
            .unwrap();

        let estados: Vec<String> = db
            .conn()
            .prepare(
                "SELECT vr.estado FROM verification_runs vr \
                 JOIN claims c ON c.id = vr.claim_id WHERE c.job_id = ?1",
            )
            .unwrap()
            .query_map(rusqlite::params![paper_id], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(!estados.is_empty());
        assert!(
            estados.iter().all(|e| e == "unverifiable"),
            "sin texto accesible toda afirmación queda unverifiable: {estados:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn leer_informes_previos_lee_los_artefactos_en_disco() {
        let dir =
            std::env::temp_dir().join(format!("entropia-paper-previos-{}", std::process::id()));
        let seccion = dir.join("job-1").join("secciones");
        std::fs::create_dir_all(&seccion).unwrap();
        std::fs::write(
            seccion.join("cronologia.v1.md"),
            "# Cronología\n\nContenido.",
        )
        .unwrap();
        let previos = leer_informes_previos(&dir, &["job-1".to_string()]);
        assert_eq!(previos.len(), 1);
        assert!(previos[0].texto.contains("Contenido"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn la_degradacion_de_zotero_se_reporta() {
        // Sin Zotero abierto, la consulta devuelve un error descriptivo y el
        // flujo puede continuar sin bibliografía de clase 3.
        let err = crate::especialistas::zotero::consultar("http://127.0.0.1:9/items", "huelga")
            .unwrap_err();
        assert!(err.contains("Zotero no está abierto"));
        let _ = MemoriaDb::nuevo(&EstadoDb::abrir_en_memoria().unwrap());
    }
}

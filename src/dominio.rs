//! Ledger epistémico del agente (PLAN §6.1, §7.1, Fase 2).
//!
//! Job → Stage DAG → Claim → ClaimEvidence → Evidence → Source, y en paralelo
//! Claim → VerificationRuns (historial append-only). El estado epistémico de
//! un claim es **proyección del último run de verificación aceptado**. `Source`
//! es la unidad formal (kind = clase 1–4); `Evidence` es la cita concreta con
//! quote + offsets exactos; `contested` se deriva de `claim_evidence` cuando
//! una misma afirmación tiene `supports` y `contradicts` a la vez.

use rusqlite::{params, OptionalExtension};

use crate::estado::{ahora, nuevo_id, EstadoDb};

/// Clase de fuente (PLAN §3): la clase de provenance deriva de `kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaseFuente {
    /// Clase 1 — fuente primaria de EntropIA (chunk del corpus).
    EntropiaChunk,
    /// Clase 2 — resultado/interpretación previa del agente.
    AgentReport,
    /// Clase 3 — bibliografía de Zotero.
    Zotero,
    /// Clase 4 — bibliografía/información externa.
    External,
}

impl ClaseFuente {
    pub fn as_str(&self) -> &'static str {
        match self {
            ClaseFuente::EntropiaChunk => "entropia_chunk",
            ClaseFuente::AgentReport => "agent_report",
            ClaseFuente::Zotero => "zotero",
            ClaseFuente::External => "external",
        }
    }

    /// Clase de provenance 1–4.
    pub fn clase(&self) -> u8 {
        match self {
            ClaseFuente::EntropiaChunk => 1,
            ClaseFuente::AgentReport => 2,
            ClaseFuente::Zotero => 3,
            ClaseFuente::External => 4,
        }
    }
}

/// Estados epistémicos — cuatro, uno por acción distinta (PLAN §6.1).
/// `contested` no se almacena: se deriva de `claim_evidence`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstadoEpistemico {
    Supported,
    PartiallySupported,
    Contradicted,
    Unverifiable,
}

impl EstadoEpistemico {
    pub fn as_str(&self) -> &'static str {
        match self {
            EstadoEpistemico::Supported => "supported",
            EstadoEpistemico::PartiallySupported => "partially_supported",
            EstadoEpistemico::Contradicted => "contradicted",
            EstadoEpistemico::Unverifiable => "unverifiable",
        }
    }

    pub fn desde_str(s: &str) -> Option<Self> {
        match s {
            "supported" => Some(Self::Supported),
            "partially_supported" => Some(Self::PartiallySupported),
            "contradicted" => Some(Self::Contradicted),
            "unverifiable" => Some(Self::Unverifiable),
            _ => None,
        }
    }
}

/// Tipo de claim (PLAN §7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TipoClaim {
    Factual,
    Interpretive,
    Causal,
    Synthetic,
}

impl TipoClaim {
    pub fn as_str(&self) -> &'static str {
        match self {
            TipoClaim::Factual => "factual",
            TipoClaim::Interpretive => "interpretive",
            TipoClaim::Causal => "causal",
            TipoClaim::Synthetic => "synthetic",
        }
    }
}

/// Relación claim ↔ evidencia.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelacionEvidencia {
    Supports,
    Contradicts,
    Contextualizes,
    Qualifies,
}

impl RelacionEvidencia {
    pub fn as_str(&self) -> &'static str {
        match self {
            RelacionEvidencia::Supports => "supports",
            RelacionEvidencia::Contradicts => "contradicts",
            RelacionEvidencia::Contextualizes => "contextualizes",
            RelacionEvidencia::Qualifies => "qualifies",
        }
    }
}

/// Fuente formal (identidad estable).
#[derive(Debug, Clone)]
pub struct Fuente {
    pub id: String,
    pub kind: String,
    pub external_id: Option<String>,
    pub item_id: Option<String>,
    pub asset_id: Option<String>,
    pub chunk_id: Option<String>,
    pub locator: Option<String>,
    pub project: String,
    pub corpus: String,
}

/// Evidencia: cita concreta dentro de una fuente.
#[derive(Debug, Clone)]
pub struct Evidencia {
    pub id: String,
    pub source_id: String,
    pub quote: String,
    pub span_start: i64,
    pub span_end: i64,
    pub text_hash: Option<String>,
    pub quote_normalized_hash: Option<String>,
    pub locator: Option<String>,
    pub confianza: Option<f64>,
}

/// Claim con su estado epistémico proyectado.
#[derive(Debug, Clone)]
pub struct Claim {
    pub id: String,
    pub job_id: String,
    pub tipo: String,
    pub texto: String,
    pub status: Option<String>,
}

/// Un run de verificación (append-only). `obsoleto` marca los runs que la
/// invalidación automática (§6.1) dejó fuera de la proyección del claim.
#[derive(Debug, Clone)]
pub struct RunVerificacion {
    pub id: String,
    pub claim_id: String,
    pub estado: String,
    pub modelo: Option<String>,
    pub prompt_hash: Option<String>,
    pub rationale: Option<String>,
    pub error_kind: Option<String>,
    pub timestamp: i64,
    pub aceptado: bool,
    pub obsoleto: bool,
}

/// Ledger epistémico sobre `EstadoDb`.
pub struct Ledger<'a> {
    db: &'a EstadoDb,
}

impl<'a> Ledger<'a> {
    pub fn nuevo(db: &'a EstadoDb) -> Self {
        Self { db }
    }

    // ── sources ───────────────────────────────────────────────────────────

    /// Registra una fuente (identidad estable, única por kind + external_id).
    #[allow(clippy::too_many_arguments)]
    pub fn registrar_fuente(
        &self,
        kind: ClaseFuente,
        external_id: Option<&str>,
        item_id: Option<&str>,
        asset_id: Option<&str>,
        chunk_id: Option<&str>,
        locator: Option<&str>,
        project: &str,
        corpus: &str,
    ) -> Result<String, String> {
        if let Some(eid) = external_id {
            if let Some(existente) = self.fuente_por_external_id(kind, eid) {
                return Ok(existente.id);
            }
        }
        let id = nuevo_id("src");
        self.db
            .conn()
            .execute(
                "INSERT INTO sources (id, kind, external_id, item_id, asset_id, chunk_id, \
                 locator, project, corpus, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    id,
                    kind.as_str(),
                    external_id,
                    item_id,
                    asset_id,
                    chunk_id,
                    locator,
                    project,
                    corpus,
                    ahora()
                ],
            )
            .map_err(|e| e.to_string())?;
        Ok(id)
    }

    /// Fuente por kind + external_id (dedup).
    pub fn fuente_por_external_id(&self, kind: ClaseFuente, external_id: &str) -> Option<Fuente> {
        let mut stmt = self
            .db
            .conn()
            .prepare(
                "SELECT id, kind, external_id, item_id, asset_id, chunk_id, locator, project, \
                 corpus FROM sources WHERE kind = ?1 AND external_id = ?2",
            )
            .ok()?;
        stmt.query_row(params![kind.as_str(), external_id], |r| {
            Ok(Fuente {
                id: r.get(0)?,
                kind: r.get(1)?,
                external_id: r.get(2)?,
                item_id: r.get(3)?,
                asset_id: r.get(4)?,
                chunk_id: r.get(5)?,
                locator: r.get(6)?,
                project: r.get(7)?,
                corpus: r.get(8)?,
            })
        })
        .optional()
        .ok()
        .flatten()
    }

    /// Fuente por id.
    pub fn fuente(&self, id: &str) -> Option<Fuente> {
        let mut stmt = self
            .db
            .conn()
            .prepare(
                "SELECT id, kind, external_id, item_id, asset_id, chunk_id, locator, project, \
                 corpus FROM sources WHERE id = ?1",
            )
            .ok()?;
        stmt.query_row(params![id], |r| {
            Ok(Fuente {
                id: r.get(0)?,
                kind: r.get(1)?,
                external_id: r.get(2)?,
                item_id: r.get(3)?,
                asset_id: r.get(4)?,
                chunk_id: r.get(5)?,
                locator: r.get(6)?,
                project: r.get(7)?,
                corpus: r.get(8)?,
            })
        })
        .optional()
        .ok()
        .flatten()
    }

    // ── evidence ──────────────────────────────────────────────────────────

    /// Registra una evidencia. Los offsets son de **caracteres** sobre el texto
    /// de la fuente; el span debe cubrir exactamente la cita (o coincidir por
    /// normalización), si no se rechaza.
    pub fn registrar_evidencia(
        &self,
        source_id: &str,
        quote: &str,
        span_start: i64,
        span_end: i64,
        locator: Option<&str>,
        confianza: Option<f64>,
    ) -> Result<String, String> {
        if span_start > span_end {
            return Err("span inválido: inicio mayor que fin".into());
        }
        let largo = quote.chars().count() as i64;
        if span_end - span_start != largo {
            return Err(format!(
                "span inválido: la cita tiene {largo} caracteres pero el span declara {}",
                span_end - span_start
            ));
        }
        let id = nuevo_id("ev");
        let quote_normalized_hash = format!(
            "{:016x}",
            crate::repositorio::fnv1a_64(&normalizar_cita(quote).into_bytes())
        );
        self.db
            .conn()
            .execute(
                "INSERT INTO evidence (id, source_id, quote, span_start, span_end, text_hash, \
                 quote_normalized_hash, locator, confianza, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, ?8, ?9)",
                params![
                    id,
                    source_id,
                    quote,
                    span_start,
                    span_end,
                    quote_normalized_hash,
                    locator,
                    confianza,
                    ahora()
                ],
            )
            .map_err(|e| e.to_string())?;
        Ok(id)
    }

    /// Evidencia por id.
    pub fn evidencia(&self, id: &str) -> Option<Evidencia> {
        let mut stmt = self
            .db
            .conn()
            .prepare(
                "SELECT id, source_id, quote, span_start, span_end, text_hash, \
                 quote_normalized_hash, locator, confianza FROM evidence WHERE id = ?1",
            )
            .ok()?;
        stmt.query_row(params![id], |r| {
            Ok(Evidencia {
                id: r.get(0)?,
                source_id: r.get(1)?,
                quote: r.get(2)?,
                span_start: r.get(3)?,
                span_end: r.get(4)?,
                text_hash: r.get(5)?,
                quote_normalized_hash: r.get(6)?,
                locator: r.get(7)?,
                confianza: r.get(8)?,
            })
        })
        .optional()
        .ok()
        .flatten()
    }

    // ── claims ────────────────────────────────────────────────────────────

    /// Registra un claim. Devuelve su id.
    pub fn registrar_claim(
        &self,
        job_id: &str,
        tipo: TipoClaim,
        texto: &str,
    ) -> Result<String, String> {
        let id = nuevo_id("claim");
        let t = ahora();
        self.db
            .conn()
            .execute(
                "INSERT INTO claims (id, job_id, type, texto, status, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?5)",
                params![id, job_id, tipo.as_str(), texto, t],
            )
            .map_err(|e| e.to_string())?;
        Ok(id)
    }

    /// Relaciona un claim con una evidencia (support/contradicts/…).
    pub fn relacionar(
        &self,
        claim_id: &str,
        evidence_id: &str,
        relation: RelacionEvidencia,
        support_strength: Option<f64>,
    ) -> Result<(), String> {
        let id = nuevo_id("ce");
        self.db
            .conn()
            .execute(
                "INSERT OR IGNORE INTO claim_evidence (id, claim_id, evidence_id, relation, \
                 support_strength) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    id,
                    claim_id,
                    evidence_id,
                    relation.as_str(),
                    support_strength
                ],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Claim por id.
    pub fn claim(&self, id: &str) -> Option<Claim> {
        let mut stmt = self
            .db
            .conn()
            .prepare("SELECT id, job_id, type, texto, status FROM claims WHERE id = ?1")
            .ok()?;
        stmt.query_row(params![id], |r| {
            Ok(Claim {
                id: r.get(0)?,
                job_id: r.get(1)?,
                tipo: r.get(2)?,
                texto: r.get(3)?,
                status: r.get(4)?,
            })
        })
        .optional()
        .ok()
        .flatten()
    }

    /// Estado epistémico proyectado de un claim (último run aceptado).
    pub fn estado_epistemico(&self, claim_id: &str) -> Option<EstadoEpistemico> {
        let status = self.claim(claim_id)?.status?;
        EstadoEpistemico::desde_str(&status)
    }

    /// `true` si el claim tiene evidencia `supports` y `contradicts` a la vez
    /// (contested se deriva, no se almacena; PLAN §6.1).
    pub fn es_contested(&self, claim_id: &str) -> bool {
        let tiene = |rel: &str| -> bool {
            self.db
                .conn()
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM claim_evidence WHERE claim_id = ?1 AND relation = ?2)",
                    params![claim_id, rel],
                    |r| r.get::<_, bool>(0),
                )
                .unwrap_or(false)
        };
        tiene("supports") && tiene("contradicts")
    }

    /// Evidencias relacionadas con un claim.
    pub fn evidencias_del_claim(&self, claim_id: &str) -> Vec<(Evidencia, String, Option<f64>)> {
        let Ok(mut stmt) = self.db.conn().prepare(
            "SELECT e.id, e.source_id, e.quote, e.span_start, e.span_end, e.text_hash, \
             e.quote_normalized_hash, e.locator, e.confianza, ce.relation, ce.support_strength \
             FROM claim_evidence ce JOIN evidence e ON e.id = ce.evidence_id \
             WHERE ce.claim_id = ?1",
        ) else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map(params![claim_id], |r| {
            Ok((
                Evidencia {
                    id: r.get(0)?,
                    source_id: r.get(1)?,
                    quote: r.get(2)?,
                    span_start: r.get(3)?,
                    span_end: r.get(4)?,
                    text_hash: r.get(5)?,
                    quote_normalized_hash: r.get(6)?,
                    locator: r.get(7)?,
                    confianza: r.get(8)?,
                },
                r.get::<_, String>(9)?,
                r.get::<_, Option<f64>>(10)?,
            ))
        }) else {
            return Vec::new();
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    // ── verification_runs ─────────────────────────────────────────────────

    /// Registra un run de verificación (append-only). Si `aceptado`, actualiza
    /// la proyección del claim. Un run nuevo no aceptado no cambia el estado.
    #[allow(clippy::too_many_arguments)]
    pub fn registrar_verificacion(
        &self,
        claim_id: &str,
        estado: EstadoEpistemico,
        modelo: Option<&str>,
        prompt_hash: Option<&str>,
        evidencia_considerada: Option<&str>,
        contraevidencia: Option<&str>,
        rationale: Option<&str>,
        error_kind: Option<&str>,
        aceptado: bool,
    ) -> Result<String, String> {
        let id = nuevo_id("vr");
        let t = ahora();
        let claim_id2 = claim_id.to_string();
        let estado2 = estado;
        self.db.con_transaccion(|conn| {
            conn.execute(
                "INSERT INTO verification_runs (id, claim_id, estado, modelo, prompt_hash, \
                 evidencia_considerada, contraevidencia, rationale, error_kind, timestamp, aceptado) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    id, claim_id2, estado2.as_str(), modelo, prompt_hash, evidencia_considerada,
                    contraevidencia, rationale, error_kind, t, aceptado as i64
                ],
            )
            .map_err(|e| e.to_string())?;
            if aceptado {
                conn.execute(
                    "UPDATE claims SET status = ?1, updated_at = ?2 WHERE id = ?3",
                    params![estado2.as_str(), t, claim_id2],
                )
                .map_err(|e| e.to_string())?;
            }
            Ok(())
        })?;
        Ok(id)
    }

    /// Runs de verificación de un claim en orden (append-only).
    pub fn runs_del_claim(&self, claim_id: &str) -> Vec<RunVerificacion> {
        let Ok(mut stmt) = self.db.conn().prepare(
            "SELECT id, claim_id, estado, modelo, prompt_hash, rationale, error_kind, \
             timestamp, aceptado, obsoleto FROM verification_runs WHERE claim_id = ?1 \
             ORDER BY timestamp, rowid",
        ) else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map(params![claim_id], |r| {
            Ok(RunVerificacion {
                id: r.get(0)?,
                claim_id: r.get(1)?,
                estado: r.get(2)?,
                modelo: r.get(3)?,
                prompt_hash: r.get(4)?,
                rationale: r.get(5)?,
                error_kind: r.get(6)?,
                timestamp: r.get(7)?,
                aceptado: r.get::<_, i64>(8)? != 0,
                obsoleto: r.get::<_, i64>(9)? != 0,
            })
        }) else {
            return Vec::new();
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    /// Último run aceptado **no obsoleto** de un claim (la verificación
    /// vigente). `None` si el claim nunca se verificó o su verificación fue
    /// invalidada por un cambio (§6.1).
    pub fn verificacion_vigente(&self, claim_id: &str) -> Option<RunVerificacion> {
        self.runs_del_claim(claim_id)
            .into_iter()
            .rev()
            .find(|r| r.aceptado && !r.obsoleto)
    }

    // ── invalidación automática (§6.1) ────────────────────────────────────

    /// Invalida la verificación vigente de un claim: marca los runs aceptados
    /// previos como obsoletos y resetea la proyección del claim. Los runs no
    /// se borran (append-only); quedan como historial.
    fn invalidar_verificacion(&self, claim_id: &str) -> Result<(), String> {
        self.db.con_transaccion(|conn| {
            conn.execute(
                "UPDATE verification_runs SET obsoleto = 1 \
                 WHERE claim_id = ?1 AND aceptado = 1 AND obsoleto = 0",
                params![claim_id],
            )
            .map_err(|e| e.to_string())?;
            conn.execute(
                "UPDATE claims SET status = NULL, updated_at = ?1 WHERE id = ?2",
                params![ahora(), claim_id],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })
    }

    /// Edita el texto de un claim. Cualquier cambio invalida la verificación
    /// vigente (PLAN §6.1): el claim vuelve a estado sin verificar.
    pub fn editar_claim(&self, claim_id: &str, nuevo_texto: &str) -> Result<(), String> {
        self.db
            .conn()
            .execute(
                "UPDATE claims SET texto = ?1, updated_at = ?2 WHERE id = ?3",
                params![nuevo_texto, ahora(), claim_id],
            )
            .map_err(|e| e.to_string())?;
        self.invalidar_verificacion(claim_id)
    }

    /// Edita la cita y los offsets de una evidencia. Invalida la verificación
    /// de **todos** los claims ligados a ella.
    pub fn editar_evidencia(
        &self,
        evidence_id: &str,
        quote: &str,
        span_start: i64,
        span_end: i64,
    ) -> Result<(), String> {
        if span_start > span_end {
            return Err("span inválido: inicio mayor que fin".into());
        }
        let largo = quote.chars().count() as i64;
        if span_end - span_start != largo {
            return Err(format!(
                "span inválido: la cita tiene {largo} caracteres pero el span declara {}",
                span_end - span_start
            ));
        }
        let quote_normalized_hash = format!(
            "{:016x}",
            crate::repositorio::fnv1a_64(&normalizar_cita(quote).into_bytes())
        );
        self.db.con_transaccion(|conn| {
            conn.execute(
                "UPDATE evidence SET quote = ?1, span_start = ?2, span_end = ?3, \
                 quote_normalized_hash = ?4 WHERE id = ?5",
                params![
                    quote,
                    span_start,
                    span_end,
                    quote_normalized_hash,
                    evidence_id
                ],
            )
            .map_err(|e| e.to_string())?;
            // Claims afectados → invalidar sus verificaciones.
            let mut stmt = conn
                .prepare("SELECT claim_id FROM claim_evidence WHERE evidence_id = ?1")
                .map_err(|e| e.to_string())?;
            let claims: Vec<String> = stmt
                .query_map(params![evidence_id], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?
                .filter_map(|r| r.ok())
                .collect();
            for claim_id in claims {
                conn.execute(
                    "UPDATE verification_runs SET obsoleto = 1 \
                     WHERE claim_id = ?1 AND aceptado = 1 AND obsoleto = 0",
                    params![claim_id],
                )
                .map_err(|e| e.to_string())?;
                conn.execute(
                    "UPDATE claims SET status = NULL, updated_at = ?1 WHERE id = ?2",
                    params![ahora(), claim_id],
                )
                .map_err(|e| e.to_string())?;
            }
            Ok(())
        })
    }

    /// Registra una nueva versión de una fuente (Zotero/externa). El cambio de
    /// versión invalida la verificación de los claims que citan evidencias de
    /// esa fuente (PLAN §6.1).
    pub fn actualizar_version_fuente(
        &self,
        source_id: &str,
        metadata: &str,
        excerpt: Option<&str>,
    ) -> Result<String, String> {
        let version_id = self.registrar_version_fuente(source_id, metadata, excerpt)?;
        self.db.con_transaccion(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT DISTINCT ce.claim_id FROM claim_evidence ce \
                     JOIN evidence e ON e.id = ce.evidence_id \
                     WHERE e.source_id = ?1",
                )
                .map_err(|e| e.to_string())?;
            let claims: Vec<String> = stmt
                .query_map(params![source_id], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?
                .filter_map(|r| r.ok())
                .collect();
            for claim_id in claims {
                conn.execute(
                    "UPDATE verification_runs SET obsoleto = 1 \
                     WHERE claim_id = ?1 AND aceptado = 1 AND obsoleto = 0",
                    params![claim_id],
                )
                .map_err(|e| e.to_string())?;
                conn.execute(
                    "UPDATE claims SET status = NULL, updated_at = ?1 WHERE id = ?2",
                    params![ahora(), claim_id],
                )
                .map_err(|e| e.to_string())?;
            }
            Ok(())
        })?;
        Ok(version_id)
    }

    // ── temporal + memoria ────────────────────────────────────────────────

    /// Registra un candidato de `document_date` para una fuente (PLAN §6.5).
    pub fn registrar_metadata_temporal(
        &self,
        source_id: &str,
        fecha: &str,
        precision: &str,
        confidence: f64,
        derivation: &str,
    ) -> Result<(), String> {
        let id = nuevo_id("tmp");
        self.db
            .conn()
            .execute(
                "INSERT INTO source_temporal_metadata (id, source_id, date, precision, \
                 confidence, derivation) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, source_id, fecha, precision, confidence, derivation],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Liga una memoria longitudinal a una evidencia (PLAN §7.2).
    pub fn ligar_memoria_evidencia(
        &self,
        memory_id: &str,
        evidence_id: &str,
        relation: &str,
    ) -> Result<(), String> {
        let id = nuevo_id("me");
        self.db
            .conn()
            .execute(
                "INSERT INTO memory_evidence (id, memory_id, evidence_id, relation) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, memory_id, evidence_id, relation],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    // ── source_versions (Fase 3, Modo 2) ─────────────────────────────────

    /// Registra un snapshot recuperable de una fuente (Zotero / externa):
    /// metadata bibliográfica congelada + excerpt cuando el texto es accesible.
    pub fn registrar_version_fuente(
        &self,
        source_id: &str,
        metadata: &str,
        excerpt: Option<&str>,
    ) -> Result<String, String> {
        let id = nuevo_id("sv");
        let content_hash = format!("{:016x}", crate::repositorio::fnv1a_64(metadata.as_bytes()));
        // Monotónico por fuente: dos versiones consecutivas nunca colisionan en
        // el UNIQUE(source_id, retrieved_at), aunque el reloj no avance entre
        // llamadas (precisión de ms o menos). `retrieved_at` = max(ahora,
        // última versión + 1).
        let ahora_ms = time::OffsetDateTime::now_utc().unix_timestamp_nanos() as i64 / 1_000_000;
        let ultimo: i64 = self
            .db
            .conn()
            .query_row(
                "SELECT COALESCE(MAX(retrieved_at), 0) FROM source_versions WHERE source_id = ?1",
                params![source_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let retrieved_at = ahora_ms.max(ultimo + 1);
        self.db
            .conn()
            .execute(
                "INSERT INTO source_versions (id, source_id, retrieved_at, content_hash, \
                 metadata, excerpt) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, source_id, retrieved_at, content_hash, metadata, excerpt],
            )
            .map_err(|e| e.to_string())?;
        Ok(id)
    }

    /// ¿La fuente tiene texto accesible (excerpt capturado)?
    pub fn fuente_tiene_texto(&self, source_id: &str) -> bool {
        self.db
            .conn()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM source_versions WHERE source_id = ?1 \
                 AND excerpt IS NOT NULL AND excerpt != '')",
                params![source_id],
                |r| r.get::<_, bool>(0),
            )
            .unwrap_or(false)
    }

    /// Clases de provenance (1–4) de las evidencias de un claim, derivadas del
    /// `kind` de sus fuentes (PLAN §3).
    pub fn clases_del_claim(&self, claim_id: &str) -> Vec<u8> {
        let Ok(mut stmt) = self.db.conn().prepare(
            "SELECT DISTINCT s.kind FROM claim_evidence ce \
             JOIN evidence e ON e.id = ce.evidence_id \
             JOIN sources s ON s.id = e.source_id WHERE ce.claim_id = ?1",
        ) else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map(params![claim_id], |r| r.get::<_, String>(0)) else {
            return Vec::new();
        };
        rows.filter_map(|r| r.ok())
            .filter_map(|k| {
                let clase = match k.as_str() {
                    "entropia_chunk" => 1,
                    "agent_report" => 2,
                    "zotero" => 3,
                    "external" => 4,
                    _ => return None,
                };
                Some(clase)
            })
            .collect()
    }
}

/// Normaliza una cita para comparación: minúsculas y espacios colapsados.
pub fn normalizar_cita(t: &str) -> String {
    t.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Convierte un offset de bytes a offset de caracteres.
pub fn char_offset(texto: &str, byte_idx: usize) -> usize {
    texto[..byte_idx.min(texto.len())].chars().count()
}

/// Span check determinista (PLAN §6.1): la cita debe aparecer en el texto de
/// la fuente exactamente en los offsets declarados (coincidencia exacta o por
/// normalización).
pub fn span_coincide(quote: &str, texto: &str, inicio: usize, fin: usize) -> bool {
    let chars: Vec<char> = texto.chars().collect();
    if inicio > fin || fin > chars.len() {
        return false;
    }
    let ventana: String = chars[inicio..fin].iter().collect();
    ventana == quote || normalizar_cita(&ventana) == normalizar_cita(quote)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_coincide_exacto_y_normalizado() {
        let texto = "La huelga comenzó el 17 de marzo de 1965.";
        let ini = char_offset(texto, texto.find("17 de marzo").unwrap());
        assert!(span_coincide("17 de marzo", texto, ini, ini + 11));
        // Normalizado: la ventana coincide ignorando mayúsculas/espacios.
        assert!(span_coincide("17  de  marzo", texto, ini, ini + 11));
        // Offsets incorrectos → no coincide.
        assert!(!span_coincide("17 de marzo", texto, ini + 1, ini + 11));
        // Fuera de rango → no coincide.
        assert!(!span_coincide("x", texto, 0, 999));
    }

    #[test]
    fn char_offset_maneja_multibyte() {
        let texto = "huelga ñandú";
        let idx = texto.find("ñandú").unwrap();
        assert_eq!(char_offset(texto, idx), 7);
    }

    #[test]
    fn la_evidencia_valida_su_span() {
        let db = EstadoDb::abrir_en_memoria().unwrap();
        let l = Ledger::nuevo(&db);
        let src = l
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
        let texto = "La huelga comenzó el 17 de marzo.";
        let ini = char_offset(texto, texto.find("17 de marzo").unwrap()) as i64;
        // Span correcto.
        let ev = l
            .registrar_evidencia(&src, "17 de marzo", ini, ini + 11, None, Some(0.9))
            .unwrap();
        assert!(!ev.is_empty());
        // Span incorrecto → error.
        let err = l
            .registrar_evidencia(&src, "17 de marzo", ini + 1, ini + 11, None, None)
            .unwrap_err();
        assert!(err.contains("span inválido"));
    }

    #[test]
    fn la_fuente_se_deduplica_por_kind_y_external_id() {
        let db = EstadoDb::abrir_en_memoria().unwrap();
        let l = Ledger::nuevo(&db);
        let a = l
            .registrar_fuente(
                ClaseFuente::EntropiaChunk,
                Some("chunk-1"),
                None,
                None,
                Some("chunk-1"),
                None,
                "p",
                "c",
            )
            .unwrap();
        let b = l
            .registrar_fuente(
                ClaseFuente::EntropiaChunk,
                Some("chunk-1"),
                None,
                None,
                Some("chunk-1"),
                None,
                "p",
                "c",
            )
            .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn el_estado_del_claim_proyecta_el_ultimo_run_aceptado() {
        let db = EstadoDb::abrir_en_memoria().unwrap();
        let l = Ledger::nuevo(&db);
        let job = l.db.conn().query_row(
            "INSERT INTO jobs (id, modo, pregunta, status, config_snapshot, project, corpus, \
             created_at, updated_at) VALUES ('job-1', 'm', 'p', 'running', '{}', 'p', 'c', 1, 1) \
             RETURNING id",
            [],
            |r| r.get::<_, String>(0),
        );
        let job_id = job.unwrap();
        let claim = l
            .registrar_claim(&job_id, TipoClaim::Factual, "La huelga comenzó en marzo.")
            .unwrap();
        assert_eq!(l.estado_epistemico(&claim), None);

        // Run no aceptado: no proyecta.
        l.registrar_verificacion(
            &claim,
            EstadoEpistemico::Supported,
            None,
            None,
            None,
            None,
            Some("borrador"),
            None,
            false,
        )
        .unwrap();
        assert_eq!(l.estado_epistemico(&claim), None);

        // Run aceptado: proyecta. Un run posterior no aceptado no lo pisa.
        l.registrar_verificacion(
            &claim,
            EstadoEpistemico::Supported,
            None,
            None,
            None,
            None,
            Some("aceptado"),
            None,
            true,
        )
        .unwrap();
        assert_eq!(
            l.estado_epistemico(&claim),
            Some(EstadoEpistemico::Supported)
        );
        l.registrar_verificacion(
            &claim,
            EstadoEpistemico::Unverifiable,
            None,
            None,
            None,
            None,
            Some("no aceptado"),
            None,
            false,
        )
        .unwrap();
        assert_eq!(
            l.estado_epistemico(&claim),
            Some(EstadoEpistemico::Supported)
        );
    }

    #[test]
    fn contested_se_deriva_de_la_evidencia() {
        let db = EstadoDb::abrir_en_memoria().unwrap();
        let l = Ledger::nuevo(&db);
        let job_id = "job-1";
        l.db.conn()
            .execute(
                "INSERT INTO jobs (id, modo, pregunta, status, config_snapshot, project, corpus, \
                 created_at, updated_at) VALUES ('job-1', 'm', 'p', 'running', '{}', 'p', 'c', 1, 1)",
                [],
            )
            .unwrap();
        let claim = l
            .registrar_claim(job_id, TipoClaim::Factual, "La huelga fue en marzo.")
            .unwrap();
        let src = l
            .registrar_fuente(
                ClaseFuente::EntropiaChunk,
                Some("c1"),
                None,
                None,
                None,
                None,
                "p",
                "c",
            )
            .unwrap();
        let e1 = l
            .registrar_evidencia(&src, "marzo", 0, 5, None, None)
            .unwrap();
        let e2 = l
            .registrar_evidencia(&src, "abril", 0, 5, None, None)
            .unwrap();
        l.relacionar(&claim, &e1, RelacionEvidencia::Supports, Some(0.8))
            .unwrap();
        assert!(!l.es_contested(&claim));
        l.relacionar(&claim, &e2, RelacionEvidencia::Contradicts, Some(0.8))
            .unwrap();
        assert!(l.es_contested(&claim));
    }

    fn job_de_prueba(l: &Ledger) -> String {
        l.db.conn()
            .query_row(
                "INSERT INTO jobs (id, modo, pregunta, status, config_snapshot, project, corpus, \
                 created_at, updated_at) VALUES ('job-edit', 'm', 'p', 'running', '{}', 'p', 'c', 1, 1) \
                 RETURNING id",
                [],
                |r| r.get::<_, String>(0),
            )
            .unwrap()
    }

    #[test]
    fn editar_el_claim_invalida_la_verificacion_vigente() {
        let db = EstadoDb::abrir_en_memoria().unwrap();
        let l = Ledger::nuevo(&db);
        let job_id = job_de_prueba(&l);
        let claim = l
            .registrar_claim(&job_id, TipoClaim::Factual, "La huelga comenzó en marzo.")
            .unwrap();
        l.registrar_verificacion(
            &claim,
            EstadoEpistemico::Supported,
            None,
            None,
            None,
            None,
            Some("aceptado"),
            None,
            true,
        )
        .unwrap();
        assert_eq!(
            l.estado_epistemico(&claim),
            Some(EstadoEpistemico::Supported)
        );
        assert!(l.verificacion_vigente(&claim).is_some());

        // Cambio en el texto del claim → la verificación vigente queda obsoleta.
        l.editar_claim(&claim, "La huelga comenzó el 17 de marzo de 1965.")
            .unwrap();
        assert_eq!(
            l.estado_epistemico(&claim),
            None,
            "la proyección se resetea"
        );
        let run = l.verificacion_vigente(&claim);
        assert!(run.is_none(), "no hay verificación vigente tras editar");
        let runs = l.runs_del_claim(&claim);
        assert_eq!(runs.len(), 1, "los runs no se borran (append-only)");
        assert!(runs[0].aceptado);
        assert!(runs[0].obsoleto, "el run aceptado previo queda obsoleto");

        // Una verificación nueva (aceptada) vuelve a proyectar.
        l.registrar_verificacion(
            &claim,
            EstadoEpistemico::Supported,
            None,
            None,
            None,
            None,
            Some("re-verificado"),
            None,
            true,
        )
        .unwrap();
        assert_eq!(
            l.estado_epistemico(&claim),
            Some(EstadoEpistemico::Supported)
        );
        assert!(l.verificacion_vigente(&claim).is_some());
    }

    #[test]
    fn editar_la_evidencia_invalida_los_claims_ligados() {
        let db = EstadoDb::abrir_en_memoria().unwrap();
        let l = Ledger::nuevo(&db);
        let job_id = job_de_prueba(&l);
        let src = l
            .registrar_fuente(
                ClaseFuente::EntropiaChunk,
                Some("c1"),
                None,
                None,
                None,
                None,
                "p",
                "c",
            )
            .unwrap();
        let ev = l
            .registrar_evidencia(&src, "marzo", 0, 5, None, None)
            .unwrap();
        let claim = l
            .registrar_claim(&job_id, TipoClaim::Factual, "La huelga fue en marzo.")
            .unwrap();
        l.relacionar(&claim, &ev, RelacionEvidencia::Supports, Some(0.8))
            .unwrap();
        l.registrar_verificacion(
            &claim,
            EstadoEpistemico::Supported,
            None,
            None,
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap();
        assert_eq!(
            l.estado_epistemico(&claim),
            Some(EstadoEpistemico::Supported)
        );

        // Cambio de cita y offsets → el claim ligado pierde su verificación.
        l.editar_evidencia(&ev, "abril", 0, 5).unwrap();
        assert_eq!(l.estado_epistemico(&claim), None);
        assert!(l.verificacion_vigente(&claim).is_none());
    }

    #[test]
    fn actualizar_la_version_de_la_fuente_invalida_los_claims() {
        let db = EstadoDb::abrir_en_memoria().unwrap();
        let l = Ledger::nuevo(&db);
        let job_id = job_de_prueba(&l);
        let src = l
            .registrar_fuente(
                ClaseFuente::Zotero,
                Some("z1"),
                None,
                None,
                None,
                None,
                "p",
                "c",
            )
            .unwrap();
        l.registrar_version_fuente(&src, "{\"titulo\":\"v1\"}", Some("texto v1"))
            .unwrap();
        let ev = l
            .registrar_evidencia(&src, "texto v1", 0, 8, None, None)
            .unwrap();
        let claim = l
            .registrar_claim(&job_id, TipoClaim::Factual, "El paper dice texto v1.")
            .unwrap();
        l.relacionar(&claim, &ev, RelacionEvidencia::Supports, Some(0.8))
            .unwrap();
        l.registrar_verificacion(
            &claim,
            EstadoEpistemico::Supported,
            None,
            None,
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap();
        assert_eq!(
            l.estado_epistemico(&claim),
            Some(EstadoEpistemico::Supported)
        );

        // Nueva versión de la fuente → verificación obsoleta.
        l.actualizar_version_fuente(&src, "{\"titulo\":\"v2\"}", Some("texto v2"))
            .unwrap();
        assert_eq!(l.estado_epistemico(&claim), None);
        assert!(l.verificacion_vigente(&claim).is_none());
    }

    #[test]
    fn las_versiones_consecutivas_de_una_fuente_nunca_colisionan() {
        let db = EstadoDb::abrir_en_memoria().unwrap();
        let l = Ledger::nuevo(&db);
        let src = l
            .registrar_fuente(
                ClaseFuente::Zotero,
                Some("z-mono"),
                None,
                None,
                None,
                None,
                "p",
                "c",
            )
            .unwrap();
        // Varias versiones en bucle cerrado (mismo milisegundo): el bump
        // monotónico garantiza retrieved_at estrictamente creciente y ningún
        // UNIQUE(source_id, retrieved_at) falla.
        for i in 0..50 {
            l.registrar_version_fuente(&src, &format!("{{\"v\":{i}}}"), Some("texto"))
                .unwrap();
        }
        let timestamps: Vec<i64> = db
            .conn()
            .prepare(
                "SELECT retrieved_at FROM source_versions WHERE source_id = ?1 ORDER BY retrieved_at",
            )
            .unwrap()
            .query_map(params![src], |r| r.get::<_, i64>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(timestamps.len(), 50);
        assert!(
            timestamps.windows(2).all(|w| w[0] < w[1]),
            "retrieved_at debe ser estrictamente creciente: {timestamps:?}"
        );
    }
}

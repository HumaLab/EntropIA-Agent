//! Estado persistente del agente (`estado.sqlite`), separado y escribible a
//! diferencia del corpus (PLAN §6.8, §7.1).
//!
//! El esquema está versionado (`migrations`) con migraciones incrementales
//! desde Fase 1: al integrar en Tauri con investigaciones reales la base no se
//! recrea. Cada tabla nace con su primer escritor (§7.1): la migración 1 crea
//! las tablas de Fase 1 (jobs, stages, dependencias, queries, llm_calls,
//! job_events, human_decisions, artifacts, memories); las tablas epistémicas
//! (sources, evidence, claims, …) entran en la migración 2 con su primer
//! escritor (Fase 2).

use std::path::Path;

use rusqlite::{params, Connection};

/// Versión de esquema actual del estado del agente.
pub const VERSION_ESQUEMA: i64 = 1;

/// Migraciones incrementales: índice i → versión i+1.
///
/// Fase 1 (§7.1): jobs, stages, stage_dependencies, queries, llm_calls,
/// job_events, human_decisions, artifacts y las tablas de memoria longitudinal
/// (memories + memory_relations + memories_fts con sus triggers).
const MIGRACIONES: &[&str] = &[
    // Migración 1 — Fase 1: estado persistente y trabajos largos.
    r#"
CREATE TABLE jobs (
  id TEXT PRIMARY KEY,
  modo TEXT NOT NULL,
  pregunta TEXT NOT NULL,
  plan_json TEXT,
  status TEXT NOT NULL CHECK(status IN ('planned','running','paused','awaiting_human','done','failed')),
  close_reason TEXT CHECK(close_reason IN ('completed','cancelled','budget_exhausted','blocked')),
  costo_acumulado REAL NOT NULL DEFAULT 0,
  max_cost REAL,
  max_llm_calls INTEGER,
  config_snapshot TEXT NOT NULL,
  corpus_snapshot_id TEXT,
  project TEXT NOT NULL,
  corpus TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE stages (
  id TEXT PRIMARY KEY,
  job_id TEXT NOT NULL REFERENCES jobs(id),
  tipo TEXT NOT NULL,
  titulo TEXT,
  status TEXT NOT NULL DEFAULT 'pending'
    CHECK(status IN ('pending','in_progress','completed','reused','failed')),
  artifact_path TEXT,
  checkpoint TEXT,
  started_at INTEGER,
  completed_at INTEGER
);
CREATE INDEX idx_stages_job ON stages(job_id);

CREATE TABLE stage_dependencies (
  stage_id TEXT NOT NULL REFERENCES stages(id),
  depends_on_stage_id TEXT NOT NULL REFERENCES stages(id),
  PRIMARY KEY (stage_id, depends_on_stage_id)
);

CREATE TABLE queries (
  id TEXT PRIMARY KEY,
  job_id TEXT NOT NULL REFERENCES jobs(id),
  stage_id TEXT,
  consulta TEXT NOT NULL,
  filtros TEXT,
  resultados INTEGER,
  reformulated_from TEXT,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_queries_job ON queries(job_id);

CREATE TABLE llm_calls (
  id TEXT PRIMARY KEY,
  job_id TEXT NOT NULL REFERENCES jobs(id),
  stage_id TEXT,
  rol TEXT,
  modelo TEXT,
  tokens_input INTEGER,
  tokens_output INTEGER,
  costo REAL,
  latencia_ms INTEGER,
  reintentos INTEGER,
  error TEXT,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_llm_calls_job ON llm_calls(job_id);

CREATE TABLE job_events (
  id TEXT PRIMARY KEY,
  job_id TEXT NOT NULL REFERENCES jobs(id),
  stage_id TEXT,
  tipo TEXT NOT NULL,
  payload TEXT,
  timestamp INTEGER NOT NULL
);
CREATE INDEX idx_job_events_job ON job_events(job_id, timestamp);

CREATE TABLE human_decisions (
  id TEXT PRIMARY KEY,
  job_id TEXT NOT NULL REFERENCES jobs(id),
  stage_id TEXT,
  alcance TEXT,
  costo_estimado REAL,
  decision TEXT NOT NULL,
  timestamp INTEGER NOT NULL
);

CREATE TABLE artifacts (
  id TEXT PRIMARY KEY,
  job_id TEXT NOT NULL REFERENCES jobs(id),
  tipo TEXT NOT NULL,
  path TEXT NOT NULL,
  padre TEXT,
  version INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_artifacts_job ON artifacts(job_id);

CREATE TABLE memories (
  id           TEXT PRIMARY KEY,
  title        TEXT NOT NULL,
  type         TEXT NOT NULL CHECK(type IN ('decision','finding','question',
                'hypothesis','interpretation','learning')),
  content      TEXT NOT NULL,
  project      TEXT NOT NULL,
  topic_key    TEXT,
  session_id   TEXT,
  created_at   INTEGER NOT NULL,
  updated_at   INTEGER NOT NULL
);

-- FTS5 de contenido externo: se puebla solo con los triggers siguientes.
CREATE VIRTUAL TABLE memories_fts USING fts5(title, content, content='memories',
  content_rowid='rowid', tokenize='unicode61 remove_diacritics 1');

CREATE TRIGGER memories_ai AFTER INSERT ON memories BEGIN
  INSERT INTO memories_fts(rowid, title, content) VALUES (new.rowid, new.title, new.content);
END;
CREATE TRIGGER memories_ad AFTER DELETE ON memories BEGIN
  INSERT INTO memories_fts(memories_fts, rowid, title, content)
    VALUES ('delete', old.rowid, old.title, old.content);
END;
CREATE TRIGGER memories_au AFTER UPDATE ON memories BEGIN
  INSERT INTO memories_fts(memories_fts, rowid, title, content)
    VALUES ('delete', old.rowid, old.title, old.content);
  INSERT INTO memories_fts(rowid, title, content) VALUES (new.rowid, new.title, new.content);
END;

CREATE TABLE memory_relations (
  id              TEXT PRIMARY KEY,
  source_id       TEXT NOT NULL REFERENCES memories(id),
  target_id       TEXT NOT NULL REFERENCES memories(id),
  relation        TEXT CHECK(relation IN ('related','compatible','scoped',
                    'conflicts_with','supersedes','not_conflict')),
  judgment_status TEXT NOT NULL DEFAULT 'pending'
                    CHECK(judgment_status IN ('pending','judged')),
  judged_at       INTEGER
);
CREATE INDEX idx_memory_relations_source ON memory_relations(source_id);
"#,
];

/// Base de estado del agente (escritura).
pub struct EstadoDb {
    conn: Connection,
}

impl EstadoDb {
    /// Abre (o crea) la base de estado en `path`, aplicando las migraciones
    /// pendientes. Crea los directorios padres si faltan.
    pub fn abrir(path: &str) -> Result<Self, String> {
        if let Some(dir) = Path::new(path).parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| format!("No se pudo crear el directorio de estado: {e}"))?;
            }
        }
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        let mut db = Self { conn };
        db.migrar()?;
        Ok(db)
    }

    /// Base de estado en memoria (tests).
    pub fn abrir_en_memoria() -> Result<Self, String> {
        let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
        let mut db = Self { conn };
        db.migrar()?;
        Ok(db)
    }

    /// Versión de esquema aplicada (0 si la base es nueva).
    pub fn version_esquema(&self) -> i64 {
        let Ok(mut stmt) = self
            .conn
            .prepare("SELECT COALESCE(MAX(version), 0) FROM migrations")
        else {
            return 0;
        };
        stmt.query_row([], |r| r.get::<_, i64>(0)).unwrap_or(0)
    }

    /// Aplica las migraciones pendientes, cada una en su transacción.
    fn migrar(&mut self) -> Result<(), String> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS migrations (
                   version INTEGER PRIMARY KEY,
                   applied_at INTEGER NOT NULL
                 );",
            )
            .map_err(|e| e.to_string())?;
        let version = self.version_esquema();
        for (i, sql) in MIGRACIONES.iter().enumerate() {
            let target = i as i64 + 1;
            if target <= version {
                continue;
            }
            let tx = self.conn.transaction().map_err(|e| e.to_string())?;
            tx.execute_batch(sql).map_err(|e| e.to_string())?;
            let ahora = ahora();
            tx.execute(
                "INSERT INTO migrations (version, applied_at) VALUES (?1, ?2)",
                params![target, ahora],
            )
            .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Conexión cruda (uso interno de los módulos de jobs/memoria).
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Ejecuta `f` dentro de una transacción (BEGIN IMMEDIATE … COMMIT), con
    /// rollback automático ante error. `Connection` solo permite operaciones
    /// `&self`, así que la transacción se maneja a mano sobre la conexión
    /// compartida (uso de un solo hilo por base).
    pub(crate) fn con_transaccion<F, T>(&self, f: F) -> Result<T, String>
    where
        F: FnOnce(&Connection) -> Result<T, String>,
    {
        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| e.to_string())?;
        match f(&self.conn) {
            Ok(valor) => {
                self.conn
                    .execute_batch("COMMIT")
                    .map_err(|e| e.to_string())?;
                Ok(valor)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }
}

/// Marca de tiempo epoch (segundos) para las filas de estado.
pub fn ahora() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

/// Identificador nuevo con prefijo: timestamp + pid + contador atómico.
pub fn nuevo_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CONTADOR: AtomicU64 = AtomicU64::new(0);
    let n = CONTADOR.fetch_add(1, Ordering::SeqCst);
    format!("{prefix}-{}-{n}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_base_nueva_aplica_la_migracion_1() {
        let db = EstadoDb::abrir_en_memoria().unwrap();
        assert_eq!(db.version_esquema(), 1);
    }

    #[test]
    fn reabrir_no_replica_migraciones() {
        let path = std::env::temp_dir().join(format!(
            "entropia-estado-test-{}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = EstadoDb::abrir(path.to_str().unwrap()).unwrap();
        assert_eq!(db.version_esquema(), 1);
        drop(db);
        let db2 = EstadoDb::abrir(path.to_str().unwrap()).unwrap();
        assert_eq!(db2.version_esquema(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn las_tablas_de_fase_1_existen() {
        let db = EstadoDb::abrir_en_memoria().unwrap();
        let tablas: Vec<String> = db
            .conn()
            .prepare(
                "SELECT name FROM sqlite_master WHERE type IN ('table','view') \
                 AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        for tabla in [
            "jobs",
            "stages",
            "stage_dependencies",
            "queries",
            "llm_calls",
            "job_events",
            "human_decisions",
            "artifacts",
            "memories",
            "memories_fts",
            "memory_relations",
            "migrations",
        ] {
            assert!(
                tablas.contains(&tabla.to_string()),
                "falta la tabla {tabla}"
            );
        }
        // Las tablas epistémicas NO existen todavía (nacen en Fase 2).
        for tabla in ["sources", "evidence", "claims", "verification_runs"] {
            assert!(
                !tablas.contains(&tabla.to_string()),
                "{tabla} no debería existir aún"
            );
        }
    }

    #[test]
    fn los_ids_nuevos_son_unicos_y_prefijados() {
        let a = nuevo_id("job");
        let b = nuevo_id("job");
        assert!(a.starts_with("job-"));
        assert_ne!(a, b);
    }
}

//! Informe por secciones versionadas (PLAN §6.7, Fase 2).
//!
//! `actualizar_informe` trabaja sobre secciones (`section v1→v2→v3`) con
//! provenance y diff por versión: un hallazgo tardío puede reabrir y regenerar
//! una sección **sin regenerar el informe completo**. Cada versión es un
//! archivo `secciones/{seccion_id}.v{n}.md` y una fila en `artifacts` con su
//! versión.

use std::fs;
use std::path::{Path, PathBuf};

use crate::estado::{ahora, nuevo_id, EstadoDb};
use crate::trabajos::MotorTrabajos;

/// Una sección versionada del informe.
#[derive(Debug, Clone)]
pub struct SeccionInforme {
    pub id: String,
    pub titulo: String,
    pub contenido: String,
    pub version: i64,
    /// Ids de evidencia que sustentan la sección (provenance).
    pub provenance: Vec<String>,
}

/// Maneja las secciones versionadas de un informe sobre disco + `artifacts`.
pub struct InformeSecciones<'a> {
    db: &'a EstadoDb,
    dir: PathBuf,
}

impl<'a> InformeSecciones<'a> {
    pub fn nuevo(db: &'a EstadoDb, dir: &Path) -> Self {
        Self {
            db,
            dir: dir.to_path_buf(),
        }
    }

    /// Guarda una sección. Si la sección ya existe, genera una versión nueva
    /// (v+1) y deja la anterior intacta; si no, es la v1. Registra el
    /// artefacto versionado en `artifacts`.
    pub fn guardar_seccion(
        &self,
        job_id: &str,
        seccion: &SeccionInforme,
    ) -> Result<PathBuf, String> {
        let version = self.version_actual(job_id, &seccion.id).max(1);
        let nueva_version = if self.leer_seccion(job_id, &seccion.id).is_some() {
            version + 1
        } else {
            1
        };
        let dir_seccion = self.dir.join(job_id).join("secciones");
        fs::create_dir_all(&dir_seccion)
            .map_err(|e| format!("No se pudo crear el directorio de secciones: {e}"))?;
        let ruta = dir_seccion.join(format!("{}.v{nueva_version}.md", seccion.id));
        let contenido = render_seccion(seccion, nueva_version);
        fs::write(&ruta, contenido).map_err(|e| format!("No se pudo escribir la sección: {e}"))?;

        // Fila de artefacto versionada (provenance en el contenido).
        let m = MotorTrabajos::nuevo(self.db);
        let artefacto_id = nuevo_id("art");
        self.db
            .conn()
            .execute(
                "INSERT INTO artifacts (id, job_id, tipo, path, padre, version, created_at, content_json) \
                 VALUES (?1, ?2, 'seccion', ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    artefacto_id,
                    job_id,
                    ruta.to_str().unwrap_or(""),
                    seccion.id,
                    nueva_version,
                    ahora(),
                    contenido_json(seccion, nueva_version, &ruta)
                ],
            )
            .map_err(|e| e.to_string())?;
        m.registrar_evento(
            job_id,
            Some(&seccion.id),
            "seccion_versionada",
            Some(&format!(
                "{{\"seccion\":\"{}\",\"version\":{nueva_version},\"provenance\":[{}]}}",
                seccion.id,
                seccion
                    .provenance
                    .iter()
                    .map(|p| format!("\"{p}\""))
                    .collect::<Vec<_>>()
                    .join(",")
            )),
        )?;
        Ok(ruta)
    }
    /// Versión actual de una sección (0 si nunca se guardó).
    pub fn version_actual(&self, job_id: &str, seccion_id: &str) -> i64 {
        self.db
            .conn()
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM artifacts \
                 WHERE job_id = ?1 AND padre = ?2 AND tipo = 'seccion'",
                rusqlite::params![job_id, seccion_id],
                |r| r.get(0),
            )
            .unwrap_or(0)
    }

    /// Lee la versión más reciente de una sección desde su archivo.
    pub fn leer_seccion(&self, job_id: &str, seccion_id: &str) -> Option<SeccionInforme> {
        let version = self.version_actual(job_id, seccion_id);
        if version == 0 {
            return None;
        }
        let ruta = self
            .dir
            .join(job_id)
            .join("secciones")
            .join(format!("{seccion_id}.v{version}.md"));
        let contenido = fs::read_to_string(&ruta).ok()?;
        Some(parse_seccion(&contenido, seccion_id, version))
    }

    /// Secciones de un job en orden de creación (para ensamblar el informe).
    pub fn secciones_del_job(&self, job_id: &str) -> Vec<String> {
        let Ok(mut stmt) = self.db.conn().prepare(
            "SELECT padre FROM artifacts WHERE job_id = ?1 AND tipo = 'seccion' \
                 GROUP BY padre ORDER BY MIN(rowid)",
        ) else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map(rusqlite::params![job_id], |r| r.get::<_, String>(0)) else {
            return Vec::new();
        };
        rows.filter_map(|r| r.ok()).collect()
    }
}

/// Contenido de la fila de artefacto: quien lee el job entiende la sección sin
/// abrir el archivo, y la lectura del snapshot no se topa con un nulo.
fn contenido_json(seccion: &SeccionInforme, version: i64, ruta: &Path) -> String {
    serde_json::json!({
        "seccion": seccion.id,
        "titulo": seccion.titulo,
        "version": version,
        "provenance": seccion.provenance,
        "ruta": ruta.to_str().unwrap_or(""),
    })
    .to_string()
}

/// Render de una sección versionada con su provenance.
fn render_seccion(s: &SeccionInforme, version: i64) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "<!-- seccion: {} · version: v{version} -->\n",
        s.id
    ));
    out.push_str(&format!("## {}\n\n", s.titulo));
    out.push_str(&s.contenido);
    out.push('\n');
    if !s.provenance.is_empty() {
        out.push_str("\n**Provenance:** ");
        out.push_str(
            &s.provenance
                .iter()
                .map(|p| format!("`{p}`"))
                .collect::<Vec<_>>()
                .join(" · "),
        );
        out.push('\n');
    }
    out
}

fn parse_seccion(contenido: &str, id: &str, version: i64) -> SeccionInforme {
    let mut titulo = id.to_string();
    let mut cuerpo = contenido.to_string();
    for linea in contenido.lines() {
        if let Some(t) = linea.strip_prefix("## ") {
            titulo = t.trim().to_string();
            cuerpo = contenido.replace(linea, "").trim().to_string();
            break;
        }
    }
    let mut provenance = Vec::new();
    for linea in contenido.lines() {
        if let Some(resto) = linea.strip_prefix("**Provenance:**") {
            for tok in resto.split('`') {
                let tok = tok.trim();
                if !tok.is_empty() && !tok.contains('·') {
                    provenance.push(tok.to_string());
                }
            }
        }
    }
    SeccionInforme {
        id: id.into(),
        titulo,
        contenido: cuerpo,
        version,
        provenance,
    }
}

/// Diff de líneas simple entre dos versiones (marcadores +/- por línea).
pub fn diff_lineas(anterior: &str, nueva: &str) -> String {
    let mut out = String::new();
    let antes: Vec<&str> = anterior.lines().collect();
    let despues: Vec<&str> = nueva.lines().collect();
    let mut i = 0;
    let mut j = 0;
    while i < antes.len() || j < despues.len() {
        if i < antes.len() && j < despues.len() && antes[i] == despues[j] {
            out.push_str(&format!("  {}\n", antes[i]));
            i += 1;
            j += 1;
        } else if j < despues.len() && (i >= antes.len() || !antes.contains(&despues[j])) {
            out.push_str(&format!("+ {}\n", despues[j]));
            j += 1;
        } else if i < antes.len() {
            out.push_str(&format!("- {}\n", antes[i]));
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn las_secciones_se_versionan_sin_pisar_la_anterior() {
        let db = EstadoDb::abrir_en_memoria().unwrap();
        let dir =
            std::env::temp_dir().join(format!("entropia-secciones-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let s = InformeSecciones::nuevo(&db, &dir);
        // Job de prueba.
        db.conn()
            .execute(
                "INSERT INTO jobs (id, modo, pregunta, status, config_snapshot, project, corpus, \
                 created_at, updated_at) VALUES ('job-1', 'm', 'p', 'running', '{}', 'p', 'c', 1, 1)",
                [],
            )
            .unwrap();

        let v1 = SeccionInforme {
            id: "cronologia".into(),
            titulo: "Cronología".into(),
            contenido: "La huelga comenzó en marzo.".into(),
            version: 1,
            provenance: vec!["ev-1".into()],
        };
        let ruta1 = s.guardar_seccion("job-1", &v1).unwrap();
        assert!(ruta1.to_str().unwrap().ends_with("cronologia.v1.md"));
        assert_eq!(s.version_actual("job-1", "cronologia"), 1);

        // Hallazgo tardío: regenera la sección → v2, la v1 sigue en disco.
        let v2 = SeccionInforme {
            id: "cronologia".into(),
            titulo: "Cronología".into(),
            contenido: "La huelga comenzó el 17 de marzo de 1965.".into(),
            version: 2,
            provenance: vec!["ev-1".into(), "ev-2".into()],
        };
        let ruta2 = s.guardar_seccion("job-1", &v2).unwrap();
        assert!(ruta2.to_str().unwrap().ends_with("cronologia.v2.md"));
        assert_eq!(s.version_actual("job-1", "cronologia"), 2);

        let leida = s.leer_seccion("job-1", "cronologia").unwrap();
        assert_eq!(leida.version, 2);
        assert!(leida.contenido.contains("17 de marzo de 1965"));
        assert!(leida.provenance.contains(&"ev-2".to_string()));

        // Otras secciones no se tocan.
        let otra = SeccionInforme {
            id: "actores".into(),
            titulo: "Actores".into(),
            contenido: "El SOIP y la patronal.".into(),
            version: 1,
            provenance: vec![],
        };
        s.guardar_seccion("job-1", &otra).unwrap();
        assert_eq!(s.version_actual("job-1", "actores"), 1);
        assert_eq!(s.version_actual("job-1", "cronologia"), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn diff_marca_lineas_agregadas_y_quitadas() {
        let d = diff_lineas("a\nb\nc", "a\nc\nd");
        assert!(d.contains("  a"));
        assert!(d.contains("- b"));
        assert!(d.contains("+ d"));
    }
}

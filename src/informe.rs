//! Persistencia del informe en disco.

use std::fs;
use std::path::{Path, PathBuf};

use time::macros::format_description;

use crate::repositorio::Cobertura;

/// Escribe el informe en un archivo Markdown dentro del directorio indicado.
/// Devuelve la ruta del archivo creado.
///
/// Fase 0 (PLAN §9): el directorio es explícito (ya no se escribe relativo al
/// CWD), para que la integración Tauri pueda apuntar al dir de datos de la app.
pub fn guardar(dir: &Path, tema: &str, texto: &str) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| format!("No se pudo crear la carpeta «informes»: {e}"))?;
    let ahora =
        time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let formato = format_description!("[year]-[month]-[day]_[hour]-[minute]-[second]");
    let estampa = ahora.format(formato).map_err(|e| e.to_string())?;
    let ruta = dir.join(format!("{}_{}.md", estampa, slug(tema)));
    fs::write(&ruta, texto).map_err(|e| format!("No se pudo escribir el informe: {e}"))?;
    Ok(ruta)
}

/// Tabla de cobertura del recorte consultado (PLAN §6.5).
///
/// Todo informe abre con esta tabla. Con 255 de ~418 items reales sin chunks,
/// un informe que no declara la cobertura es engañoso aunque cada afirmación
/// esté verificada.
pub fn tabla_cobertura(c: &Cobertura) -> String {
    let mut out = String::from("## Cobertura del recorte consultado\n\n");
    out.push_str("| Colección | Items | Con chunks | Sin procesar | Chunks |\n");
    out.push_str("|---|---|---|---|---|\n");
    for col in &c.colecciones {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            col.nombre.replace('|', "\\|"),
            col.items,
            col.items_con_chunks,
            col.items_sin_procesar(),
            col.chunks
        ));
    }
    out.push_str(&format!(
        "| **Total** | **{}** | **{}** | **{}** | — |\n",
        c.items_total, c.items_con_chunks, c.items_sin_procesar
    ));
    out.push('\n');
    out
}

/// Convierte el tema en un slug apto para un nombre de archivo.
pub fn slug(tema: &str) -> String {
    let limpio: String = tema
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'á' => 'a',
            'é' => 'e',
            'í' => 'i',
            'ó' => 'o',
            'ú' => 'u',
            'ñ' => 'n',
            _ => c,
        })
        .filter(|c| c.is_ascii_alphanumeric() || *c == ' ')
        .collect();
    let recortado: String = limpio.trim().replace(' ', "-");
    let mut resultado: String = recortado.chars().take(40).collect();
    if resultado.is_empty() {
        resultado = "informe".to_string();
    }
    resultado
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repositorio::ColeccionInfo;

    #[test]
    fn slug_separa_palabras_con_guiones() {
        assert_eq!(
            slug("Conflictividad en Mar del Plata"),
            "conflictividad-en-mar-del-plata"
        );
    }

    #[test]
    fn slug_normaliza_acentos_y_enye() {
        assert_eq!(slug("Años de crisis étnica"), "anos-de-crisis-etnica");
    }

    #[test]
    fn slug_trunca_a_cuarenta_caracteres() {
        let largo = "un tema de investigacion muy muy muy muy muy muy largo";
        assert!(slug(largo).chars().count() <= 40);
    }

    #[test]
    fn slug_vacio_devuelve_informe() {
        assert_eq!(slug(""), "informe");
    }

    #[test]
    fn slug_solo_simbolos_devuelve_informe() {
        assert_eq!(slug("¿¡?¡"), "informe");
    }

    #[test]
    fn guardar_escribe_en_el_directorio_indicado() {
        let dir =
            std::env::temp_dir().join(format!("entropia-informe-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let ruta = guardar(&dir, "Tema de prueba", "# Informe\n\ntexto").unwrap();
        assert!(ruta.starts_with(&dir));
        assert!(ruta.is_file());
        let contenido = fs::read_to_string(&ruta).unwrap();
        assert!(contenido.contains("# Informe"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tabla_cobertura_renderiza_el_resumen() {
        let cobertura = Cobertura {
            items_total: 148,
            items_con_chunks: 12,
            items_sin_procesar: 136,
            colecciones: vec![ColeccionInfo {
                id: "a".into(),
                nombre: "Conflicto SOIP 1965-66".into(),
                items: 148,
                items_con_chunks: 12,
                chunks: 40,
            }],
        };
        let tabla = tabla_cobertura(&cobertura);
        assert!(tabla.contains("Conflicto SOIP 1965-66"));
        assert!(tabla.contains("| **Total** | **148** | **12** | **136** | — |"));
        assert!(tabla.contains("Cobertura del recorte consultado"));
    }
}

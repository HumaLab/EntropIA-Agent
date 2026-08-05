//! Persistencia del informe en disco.

use std::fs;
use std::path::PathBuf;

use time::macros::format_description;

/// Escribe el informe en un archivo Markdown dentro de la carpeta «informes».
/// Devuelve la ruta del archivo creado.
pub fn guardar(tema: &str, texto: &str) -> Result<PathBuf, String> {
    fs::create_dir_all("informes")
        .map_err(|e| format!("No se pudo crear la carpeta «informes»: {e}"))?;
    let ahora =
        time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let formato = format_description!("[year]-[month]-[day]_[hour]-[minute]-[second]");
    let estampa = ahora.format(formato).map_err(|e| e.to_string())?;
    let ruta = PathBuf::from("informes").join(format!("{}_{}.md", estampa, slug(tema)));
    fs::write(&ruta, texto).map_err(|e| format!("No se pudo escribir el informe: {e}"))?;
    Ok(ruta)
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
}

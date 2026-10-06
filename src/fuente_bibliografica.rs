//! Fuente bibliográfica externa: pasajes citables de una biblioteca (Zotero u
//! otro proveedor) que el workflow de investigación puede recuperar junto al
//! corpus.
//!
//! El trait es el seam de integración: el motor no conoce la biblioteca, solo
//! sabe pedir pasajes por consulta y citarlos con su ubicación (páginas o
//! párrafos). El texto de cada pasaje viaja con él: es la fuente contra la que
//! el verificador hace el span check, sin cargar ningún asset del corpus.

use serde::{Deserialize, Serialize};

/// Tipo de ubicación de un pasaje dentro de su fuente bibliográfica.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TipoUbicacion {
    /// Páginas del documento impreso.
    Paginas,
    /// Párrafos (documentos sin paginación estable).
    Parrafos,
}

/// Rango de ubicación de un pasaje en su fuente. Los extremos son inclusivos
/// y del mismo tipo: `desde == hasta` señala un solo punto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ubicacion {
    pub tipo: TipoUbicacion,
    pub desde: i64,
    pub hasta: i64,
}

impl Ubicacion {
    /// Etiqueta legible que el informe imprime después del título: «p. 3»,
    /// «pp. 3–4», «párr. 2» o «párr. 2–5».
    pub fn etiqueta(&self) -> String {
        let (uno, rango) = match self.tipo {
            TipoUbicacion::Paginas => ("p.", "pp."),
            TipoUbicacion::Parrafos => ("párr.", "párr."),
        };
        if self.desde == self.hasta {
            format!("{uno} {}", self.desde)
        } else {
            format!("{rango} {}–{}", self.desde, self.hasta)
        }
    }

    /// Localizador compacto para el ledger («3» o «3-4»), como el de una cita
    /// bibliográfica.
    pub fn localizador(&self) -> String {
        if self.desde == self.hasta {
            format!("{}", self.desde)
        } else {
            format!("{}-{}", self.desde, self.hasta)
        }
    }
}

/// Pasaje bibliográfico citable: su propio texto, su identidad en la
/// biblioteca y su ubicación dentro de la obra.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasajeBibliografico {
    /// Identidad del pasaje dentro del item bibliográfico.
    pub chunk_id: String,
    /// Identidad estable del item en la biblioteca (p. ej. la key de Zotero).
    pub item_key: String,
    pub titulo: String,
    pub autores: String,
    pub anio: Option<i64>,
    /// Biblioteca de origen (p. ej. «user:123»): declara de dónde salió.
    pub biblioteca: String,
    /// Texto del pasaje. Es la fuente de la verificación: una cita que no está
    /// en este texto no pasa el span check.
    pub texto: String,
    pub ubicacion: Option<Ubicacion>,
}

/// Proveedor de pasajes bibliográficos.
///
/// El motor lo usa como pierna de recuperación junto al corpus. Una
/// implementación concreta habla con la biblioteca que sea; debe ser usable
/// desde el hilo del workflow (`Send + Sync`) y solo devolver pasajes cuyo
/// texto pueda sostener la cita.
pub trait FuenteBibliografica: Send + Sync {
    /// Busca pasajes por consulta, hasta `limite`.
    fn buscar(&self, consulta: &str, limite: usize) -> Result<Vec<PasajeBibliografico>, String>;
}

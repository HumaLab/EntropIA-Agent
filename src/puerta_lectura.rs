//! Gateway de lectura read-only (PLAN §6.5, Fase 2).
//!
//! Reemplaza el acceso por consulta del PoC por acceso indexado y paginado:
//! filtros previos a la semántica (colección, rango de `document_date`,
//! entidad), paginación, batches con dedup, traversal de grafo
//! (`buscar_entidad`) y reporte de cobertura por recorte. Nunca escribe en el
//! corpus y nunca devuelve chunks de colecciones excluidas.

use crate::fechas::{fecha_desde_titulo, CandidatoFecha};
use crate::repositorio::{Cobertura, RepositorioSqlite};

/// Filtros previos a la semántica de una búsqueda.
#[derive(Debug, Clone, Default)]
pub struct Filtros {
    pub coleccion: Option<String>,
    /// Rango de `document_date` (anio, mes, dia) inclusive.
    pub desde: Option<(i64, i64, i64)>,
    pub hasta: Option<(i64, i64, i64)>,
    /// Valor de entidad (persona/institución/lugar) presente en el item.
    pub entidad: Option<String>,
    pub limite: usize,
    pub offset: usize,
}

impl Filtros {
    pub fn nuevo() -> Self {
        Self {
            limite: 20,
            offset: 0,
            ..Default::default()
        }
    }
}

/// Una fuente recuperada por el gateway.
#[derive(Debug, Clone)]
pub struct FuenteRecuperada {
    pub chunk_id: String,
    pub item_id: String,
    pub asset_id: String,
    pub item_titulo: String,
    pub coleccion: String,
    pub texto: String,
    pub fecha: Option<CandidatoFecha>,
}

/// Página de resultados con su total (para paginación).
#[derive(Debug, Clone)]
pub struct PaginaFuentes {
    pub fuentes: Vec<FuenteRecuperada>,
    pub total: usize,
    pub offset: usize,
}

/// Nodo del grafo de entidades: la entidad, sus items, sus triples y los
/// chunks ligados (pierna de traversal, PLAN §6.5).
#[derive(Debug, Clone)]
pub struct NodoEntidad {
    pub entidad: String,
    pub tipo: String,
    pub items: Vec<(String, String, String)>,
    pub triples: Vec<(String, String, String)>,
    pub chunks: Vec<FuenteRecuperada>,
}

/// Candidato previo a paginación de una consulta por filtros.
struct Candidato {
    chunk_id: String,
    item_id: String,
    asset_id: String,
    item_titulo: String,
    coleccion: String,
    texto: String,
    fecha: Option<CandidatoFecha>,
}

/// Busca fuentes con filtros previos a la semántica y paginación.
///
/// El filtro de fecha se aplica en Rust (document_date se computa del título
/// con `fechas.rs`, no vive en el corpus), después de acotar por SQL con
/// colección y entidad; la paginación recae sobre el conjunto ya filtrado.
pub fn buscar_con_filtros(repo: &RepositorioSqlite, filtros: &Filtros) -> PaginaFuentes {
    let candidatos = candidatos_sql(repo, filtros);
    let candidatos: Vec<Candidato> = candidatos
        .into_iter()
        .filter(|c| dentro_de_rango(&c.fecha, filtros))
        .collect();
    let total = candidatos.len();
    let pagina: Vec<FuenteRecuperada> = candidatos
        .into_iter()
        .skip(filtros.offset)
        .take(filtros.limite)
        .map(|c| FuenteRecuperada {
            chunk_id: c.chunk_id,
            item_id: c.item_id,
            asset_id: c.asset_id,
            item_titulo: c.item_titulo,
            coleccion: c.coleccion,
            texto: c.texto,
            fecha: c.fecha,
        })
        .collect();
    PaginaFuentes {
        fuentes: pagina,
        total,
        offset: filtros.offset,
    }
}

/// Consulta SQL base con colección y entidad como filtros previos.
fn candidatos_sql(repo: &RepositorioSqlite, filtros: &Filtros) -> Vec<Candidato> {
    let (clausula, nombres) = repo.clausula_no_excluidas_pub();
    let mut sql = format!(
        "SELECT rc.id, rc.item_id, rc.asset_id, rc.text_content, i.title, c.name \
         FROM rag_chunks rc \
         JOIN items i ON i.id = rc.item_id \
         JOIN collections c ON c.id = i.collection_id \
         WHERE {clausula}"
    );
    let mut parametros: Vec<&dyn rusqlite::ToSql> =
        nombres.iter().map(|n| n as &dyn rusqlite::ToSql).collect();
    if let Some(col) = &filtros.coleccion {
        sql.push_str(" AND c.name = ?");
        parametros.push(col as &dyn rusqlite::ToSql);
    }
    if let Some(ent) = &filtros.entidad {
        sql.push_str(
            " AND i.id IN (SELECT e.item_id FROM entities e WHERE e.value LIKE '%' || ? || '%')",
        );
        parametros.push(ent as &dyn rusqlite::ToSql);
    }
    sql.push_str(" ORDER BY i.title, rc.chunk_ordinal");
    let tope = 5000;
    sql.push_str(&format!(" LIMIT {tope}"));

    let Ok(mut stmt) = repo.prepare_pub(&sql) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map(rusqlite::params_from_iter(parametros), |r| {
        Ok(Candidato {
            chunk_id: r.get(0)?,
            item_id: r.get(1)?,
            asset_id: r.get(2)?,
            texto: r.get(3)?,
            item_titulo: r.get(4)?,
            coleccion: r.get(5)?,
            fecha: None,
        })
    }) else {
        return Vec::new();
    };
    let mut out: Vec<Candidato> = rows.filter_map(|r| r.ok()).collect();
    // document_date por item (capa por capa de fechas.rs), cacheado por item.
    let mut cache_fechas = std::collections::HashMap::<String, Option<CandidatoFecha>>::new();
    for c in &mut out {
        let fecha = cache_fechas
            .entry(c.item_id.clone())
            .or_insert_with(|| fecha_desde_titulo(&c.item_titulo, &c.coleccion))
            .clone();
        c.fecha = fecha;
    }
    out
}

fn dentro_de_rango(fecha: &Option<CandidatoFecha>, filtros: &Filtros) -> bool {
    let Some(cand) = fecha else {
        // Sin fecha: solo pasa si no se pidió rango.
        return filtros.desde.is_none() && filtros.hasta.is_none();
    };
    let Some(f) = &cand.fecha else {
        return filtros.desde.is_none() && filtros.hasta.is_none();
    };
    let (anio, mes, dia) = (f.anio, f.mes.unwrap_or(0), f.dia.unwrap_or(0));
    let valor = (anio, mes, dia);
    if let Some(d) = filtros.desde {
        if valor < d {
            return false;
        }
    }
    if let Some(h) = filtros.hasta {
        if valor > h {
            return false;
        }
    }
    true
}

/// Recupera un nodo de entidad: la entidad, sus items, sus triples y los
/// chunks ligados (PLAN §6.5, pierna de traversal).
pub fn buscar_entidad(repo: &RepositorioSqlite, nombre: &str) -> Option<NodoEntidad> {
    let (clausula, nombres) = repo.clausula_no_excluidas_pub();
    let sql = format!(
        "SELECT e.value, e.entity_type, e.item_id, i.title, c.name \
         FROM entities e \
         JOIN items i ON i.id = e.item_id \
         JOIN collections c ON c.id = i.collection_id \
         WHERE {clausula} AND e.value LIKE '%' || ?1 || '%' ESCAPE '\\' \
         ORDER BY e.confidence DESC LIMIT 25"
    );
    let mut parametros: Vec<&dyn rusqlite::ToSql> =
        nombres.iter().map(|n| n as &dyn rusqlite::ToSql).collect();
    parametros.push(&nombre);
    let Ok(mut stmt) = repo.prepare_pub(&sql) else {
        return None;
    };
    let Ok(rows) = stmt.query_map(rusqlite::params_from_iter(parametros), |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
        ))
    }) else {
        return None;
    };
    let filas: Vec<(String, String, String, String, String)> =
        rows.filter_map(|r| r.ok()).collect();
    if filas.is_empty() {
        return None;
    }
    let entidad = filas[0].0.clone();
    let tipo = filas[0].1.clone();
    let item_ids: Vec<String> = filas.iter().map(|f| f.2.clone()).collect();
    let items: Vec<(String, String, String)> = filas
        .iter()
        .map(|f| (f.2.clone(), f.3.clone(), f.4.clone()))
        .collect();

    // Triples del nodo: los de sus items y los que mencionan la entidad.
    let mut triples = Vec::new();
    if let Ok(mut stmt) = repo.prepare_pub(
        "SELECT subject, predicate, object FROM triples \
         WHERE subject LIKE '%' || ?1 || '%' OR object LIKE '%' || ?1 || '%' \
         OR item_id IN (SELECT value FROM json_each(?2)) LIMIT 40",
    ) {
        let items_json = serde_json::to_string(&item_ids).unwrap_or_else(|_| "[]".into());
        if let Ok(rows) = stmt.query_map(rusqlite::params![nombre, items_json], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        }) {
            triples = rows.filter_map(|r| r.ok()).collect();
        }
    }

    // Chunks ligados a los items de la entidad.
    let mut chunks = Vec::new();
    if let Ok(mut stmt) = repo.prepare_pub(
        "SELECT rc.id, rc.item_id, rc.asset_id, rc.text_content, i.title, c.name \
         FROM rag_chunks rc \
         JOIN items i ON i.id = rc.item_id \
         JOIN collections c ON c.id = i.collection_id \
         WHERE c.name NOT IN (SELECT value FROM json_each(?1)) \
         AND rc.item_id IN (SELECT value FROM json_each(?2)) \
         ORDER BY rc.chunk_ordinal LIMIT 30",
    ) {
        let denylist_json = serde_json::to_string(&nombres).unwrap_or_else(|_| "[]".into());
        let items_json = serde_json::to_string(&item_ids).unwrap_or_else(|_| "[]".into());
        if let Ok(rows) = stmt.query_map(rusqlite::params![denylist_json, items_json], |r| {
            Ok(FuenteRecuperada {
                chunk_id: r.get(0)?,
                item_id: r.get(1)?,
                asset_id: r.get(2)?,
                texto: r.get(3)?,
                item_titulo: r.get(4)?,
                coleccion: r.get(5)?,
                fecha: None,
            })
        }) {
            chunks = rows.filter_map(|r| r.ok()).collect();
        }
    }

    Some(NodoEntidad {
        entidad,
        tipo,
        items,
        triples,
        chunks,
    })
}

/// Texto completo de un asset: los chunks del asset en orden de chunk_ordinal.
pub fn leer_asset(repo: &RepositorioSqlite, asset_id: &str) -> Option<String> {
    let sql = "SELECT text_content FROM rag_chunks WHERE asset_id = ?1 ORDER BY chunk_ordinal";
    let mut stmt = repo.prepare_pub(sql).ok()?;
    let rows = stmt
        .query_map(rusqlite::params![asset_id], |r| r.get::<_, String>(0))
        .ok()?;
    let textos: Vec<String> = rows.filter_map(|r| r.ok()).collect();
    if textos.is_empty() {
        None
    } else {
        Some(textos.join("\n"))
    }
}

/// Ruta del asset original + página para verificación humana (sin VLM).
pub fn mostrar_fuente(repo: &RepositorioSqlite, item_id: &str) -> Vec<(String, Option<i64>)> {
    let mut stmt = match repo
        .prepare_pub("SELECT path, page_number FROM assets WHERE item_id = ?1 ORDER BY sort_index")
    {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt
        .query_map(rusqlite::params![item_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?))
        })
        .ok();
    match rows {
        Some(iter) => iter.filter_map(|r| r.ok()).collect(),
        None => Vec::new(),
    }
}

/// Cobertura del recorte consultado (PLAN §6.5): por colección o total.
pub fn cobertura_recorte(repo: &RepositorioSqlite, coleccion: Option<&str>) -> Cobertura {
    let mut cobertura = repo.cobertura();
    if let Some(nombre) = coleccion {
        cobertura.colecciones.retain(|c| c.nombre == nombre);
        if let Some(col) = cobertura.colecciones.first() {
            cobertura.items_total = col.items;
            cobertura.items_con_chunks = col.items_con_chunks;
            cobertura.items_sin_procesar = col.items_sin_procesar();
        }
    }
    cobertura
}

/// Dedup de fuentes por item + colección (agregación de evidencia, §6.5).
pub fn deduplicar(fuentes: Vec<FuenteRecuperada>) -> Vec<FuenteRecuperada> {
    let mut vistos = std::collections::HashSet::new();
    let mut out = Vec::new();
    for f in fuentes {
        let clave = format!("{}|{}", f.item_id, f.coleccion);
        if vistos.insert(clave) {
            out.push(f);
        }
    }
    out
}

/// Modo batch: acumula los resultados de varias consultas con dedup (PLAN
/// §6.5: el worker acumula cientos de chunks redundantes → clusters de
/// evidencia).
pub fn acumular_batch(paginas: Vec<PaginaFuentes>) -> Vec<FuenteRecuperada> {
    let todas: Vec<FuenteRecuperada> = paginas.into_iter().flat_map(|p| p.fuentes).collect();
    deduplicar(todas)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repositorio::RepositorioSqlite;

    fn repo() -> RepositorioSqlite {
        RepositorioSqlite::abrir(crate::tests_comunes::corpus_sintetico().to_str().unwrap())
            .unwrap()
    }

    #[test]
    fn filtrar_por_coleccion() {
        let r = repo();
        let pagina = buscar_con_filtros(
            &r,
            &Filtros {
                coleccion: Some("Conflicto SOIP 1965-66".into()),
                limite: 20,
                offset: 0,
                ..Default::default()
            },
        );
        assert_eq!(pagina.total, 3);
        for f in &pagina.fuentes {
            assert_eq!(f.coleccion, "Conflicto SOIP 1965-66");
        }
    }

    #[test]
    fn filtrar_por_rango_de_fecha() {
        let r = repo();
        // item-1 = «65-03-17-a» (1965-03-17), item-2 = «65-03-20-b» (1965-03-20).
        let pagina = buscar_con_filtros(
            &r,
            &Filtros {
                desde: Some((1965, 3, 18)),
                hasta: Some((1965, 3, 31)),
                limite: 20,
                offset: 0,
                ..Default::default()
            },
        );
        assert_eq!(pagina.total, 1);
        assert_eq!(pagina.fuentes[0].item_titulo, "65-03-20-b");
    }

    #[test]
    fn paginar() {
        let r = repo();
        let p1 = buscar_con_filtros(
            &r,
            &Filtros {
                limite: 1,
                offset: 0,
                ..Default::default()
            },
        );
        let p2 = buscar_con_filtros(
            &r,
            &Filtros {
                limite: 1,
                offset: 1,
                ..Default::default()
            },
        );
        assert_eq!(p1.fuentes.len(), 1);
        assert_eq!(p2.fuentes.len(), 1);
        assert_ne!(p1.fuentes[0].chunk_id, p2.fuentes[0].chunk_id);
        assert_eq!(p1.total, 3);
    }

    #[test]
    fn nunca_devuelve_chunks_de_colecciones_excluidas() {
        let r = repo();
        let pagina = buscar_con_filtros(&r, &Filtros::nuevo());
        for f in &pagina.fuentes {
            assert!(!crate::configuracion::colecciones_excluidas().contains(&f.coleccion));
        }
    }

    #[test]
    fn dedup_agrega_por_item() {
        let duplicadas = vec![
            FuenteRecuperada {
                chunk_id: "a".into(),
                item_id: "i1".into(),
                asset_id: "x".into(),
                item_titulo: "t".into(),
                coleccion: "c".into(),
                texto: "t1".into(),
                fecha: None,
            },
            FuenteRecuperada {
                chunk_id: "b".into(),
                item_id: "i1".into(),
                asset_id: "x".into(),
                item_titulo: "t".into(),
                coleccion: "c".into(),
                texto: "t2".into(),
                fecha: None,
            },
        ];
        let unicas = deduplicar(duplicadas);
        assert_eq!(unicas.len(), 1);
    }

    #[test]
    fn cobertura_por_coleccion() {
        let r = repo();
        let c = cobertura_recorte(&r, Some("Conflicto SOIP 1965-66"));
        assert_eq!(c.items_total, 3);
        assert_eq!(c.items_sin_procesar, 1);
    }
}

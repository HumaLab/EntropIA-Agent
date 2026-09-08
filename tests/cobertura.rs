//! Tests de integración de Fase 0 (PLAN §9): denylist de colecciones de
//! prueba y cobertura declarada, sobre una copia sintética del corpus.

mod common;

use entropia_agent::repositorio::RepositorioSqlite;

#[test]
fn la_denylist_deja_fuera_las_colecciones_de_prueba_de_los_listados() {
    let repo =
        RepositorioSqlite::abrir(common::crear_corpus_sintetico().to_str().unwrap()).unwrap();
    let colecciones = repo.listar_colecciones();
    let nombres: Vec<&str> = colecciones.iter().map(|c| c.nombre.as_str()).collect();
    assert_eq!(nombres.len(), 3);
    assert!(nombres.contains(&"Conflicto SOIP 1965-66"));
    assert!(nombres.contains(&"Volantes, Panfletos, etc."));
    assert!(nombres.contains(&"SOIP 1961"));
    for excluida in entropia_agent::configuracion::colecciones_excluidas() {
        assert!(
            !nombres.contains(&excluida.as_str()),
            "{excluida} no debería listarse"
        );
    }
}

#[test]
fn el_listado_completo_alinea_investigar_con_colecciones() {
    let repo =
        RepositorioSqlite::abrir(common::crear_corpus_sintetico().to_str().unwrap()).unwrap();
    let filtradas = repo.listar_colecciones();
    let todas = repo.listar_todas_las_colecciones();
    assert_eq!(filtradas.len(), 3);
    assert_eq!(todas.len(), filtradas.len() + 6);
    for excluida in entropia_agent::configuracion::colecciones_excluidas() {
        assert!(
            todas.iter().any(|c| c.nombre == excluida),
            "{excluida} debe aparecer en Investigar como en Colecciones"
        );
    }
}

#[test]
fn cargar_chunks_excluye_los_de_colecciones_de_prueba() {
    let repo =
        RepositorioSqlite::abrir(common::crear_corpus_sintetico().to_str().unwrap()).unwrap();
    let chunks = repo.cargar_chunks().unwrap();
    assert_eq!(chunks.len(), 2);
    for c in &chunks {
        assert_eq!(c.coleccion, "Conflicto SOIP 1965-66");
    }
}

#[test]
fn buscar_fts5_excluye_los_chunks_de_colecciones_de_prueba() {
    let repo =
        RepositorioSqlite::abrir(common::crear_corpus_sintetico().to_str().unwrap()).unwrap();
    // «huelga» aparece en chunk-1 (real) y chunk-stress-1 (de prueba).
    let ids = repo.buscar_fts5("huelga", 10);
    assert!(ids.contains(&"chunk-1".to_string()));
    assert!(!ids.contains(&"chunk-stress-1".to_string()));
}

#[test]
fn cobertura_declara_items_con_y_sin_chunks() {
    let repo =
        RepositorioSqlite::abrir(common::crear_corpus_sintetico().to_str().unwrap()).unwrap();
    let cobertura = repo.cobertura();
    // Items reales: item-1, item-2 (con chunks) e item-3 (sin chunks).
    assert_eq!(cobertura.items_total, 3);
    assert_eq!(cobertura.items_con_chunks, 2);
    assert_eq!(cobertura.items_sin_procesar, 1);
    let conflicto = cobertura
        .colecciones
        .iter()
        .find(|c| c.nombre == "Conflicto SOIP 1965-66")
        .expect("la colección real debe estar");
    assert_eq!(conflicto.items, 3);
    assert_eq!(conflicto.items_con_chunks, 2);
    assert_eq!(conflicto.items_sin_procesar(), 1);
    assert_eq!(conflicto.chunks, 2);
}

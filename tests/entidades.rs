//! Tests de integración del traversal de entidades (PLAN §6.5, step 4(c)):
//! filtro por entidad y `buscar_entidad` (nodo + vecinos → chunks ligados)
//! sobre la copia sintética del corpus con entidades y triples.

mod common;

use entropia_agent::puerta_lectura::{self, Filtros};
use entropia_agent::repositorio::RepositorioSqlite;

fn repo() -> RepositorioSqlite {
    RepositorioSqlite::abrir(common::crear_corpus_sintetico().to_str().unwrap()).unwrap()
}

#[test]
fn el_filtro_por_entidad_acota_la_recuperacion() {
    let r = repo();
    let pagina = puerta_lectura::buscar_con_filtros(
        &r,
        &Filtros {
            entidad: Some("Sindicato".into()),
            limite: 20,
            offset: 0,
            ..Default::default()
        },
    );
    assert_eq!(
        pagina.total, 1,
        "solo el chunk del item con la entidad (item-1)"
    );
    for f in &pagina.fuentes {
        assert_eq!(f.item_id, "item-1");
    }
}

#[test]
fn buscar_entidad_entrega_nodo_triples_y_chunks() {
    let r = repo();
    let nodo = puerta_lectura::buscar_entidad(&r, "Sindicato").expect("entidad presente");
    assert_eq!(nodo.tipo, "organization");
    // Vecinos: el triple sujeto → predicado → objeto del item.
    let triple = nodo
        .triples
        .iter()
        .find(|t| t.1 == "denuncia")
        .expect("el triple de prueba debe estar");
    assert_eq!(triple.0, "el Sindicato Obrero de la Industria del Pescado");
    assert_eq!(triple.2, "atropellos");
    // Chunks ligados a los items de la entidad.
    assert!(nodo.chunks.iter().any(|c| c.item_id == "item-1"));
    // El nodo nunca incluye colecciones excluidas.
    for c in &nodo.chunks {
        assert!(!entropia_agent::configuracion::colecciones_excluidas().contains(&c.coleccion));
    }
}

#[test]
fn mostrar_fuente_apunta_al_asset_real() {
    let r = repo();
    let assets = puerta_lectura::mostrar_fuente(&r, "item-1");
    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0].0, "escaneos/65-03-17-a.pdf");
    assert_eq!(assets[0].1, Some(3));
}

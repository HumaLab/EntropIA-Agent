//! Test de integración contra el corpus real (`entropia.sqlite`), que
//! verifica los criterios de aceptación de Fase 0 (PLAN §9):
//!
//! - ninguna consulta devuelve chunks de colecciones excluidas;
//! - un informe sobre «Conflicto SOIP 1965-66» declara que 136 de 148 items no
//!   están procesados.
//!
//! Se salta si la base no está presente (p. ej. en CI sin el corpus).

use entropia_agent::repositorio::RepositorioSqlite;

fn ruta_corpus() -> Option<String> {
    if let Ok(path) = std::env::var("ENTROPIA_DB_PATH") {
        if !path.trim().is_empty() && std::path::Path::new(&path).exists() {
            return Some(path);
        }
    }
    let local = std::path::Path::new("entropia.sqlite");
    if local.exists() {
        return Some(local.to_str().unwrap().to_string());
    }
    None
}

#[test]
fn el_corpus_real_no_filtra_colecciones_excluidas() {
    let Some(path) = ruta_corpus() else {
        eprintln!("sin corpus real: se salta el test de denylist");
        return;
    };
    let repo = RepositorioSqlite::abrir(&path).unwrap();

    let colecciones = repo.listar_colecciones();
    for c in &colecciones {
        assert!(
            !entropia_agent::configuracion::colecciones_excluidas().contains(&c.nombre),
            "la colección de prueba «{}» no debería listarse",
            c.nombre
        );
    }

    // «huelga» está presente tanto en colecciones reales como de prueba: ningún
    // resultado puede provenir de las excluidas.
    let ids = repo.buscar_fts5("huelga", 30);
    assert!(
        !ids.is_empty(),
        "el corpus real debería tener resultados de «huelga»"
    );
    let excluidas = entropia_agent::configuracion::colecciones_excluidas();
    for id in ids {
        let coleccion = repo
            .coleccion_de_chunk(&id)
            .unwrap_or_else(|| panic!("el chunk «{id}» debería mapear a su colección"));
        assert!(
            !excluidas.contains(&coleccion),
            "el chunk «{id}» proviene de la colección excluida «{coleccion}»"
        );
    }
}

#[test]
fn el_informe_sobre_el_conflicto_declara_su_cobertura() {
    let Some(path) = ruta_corpus() else {
        eprintln!("sin corpus real: se salta el test de cobertura");
        return;
    };
    let repo = RepositorioSqlite::abrir(&path).unwrap();
    let cobertura = repo.cobertura();

    let conflicto = cobertura
        .colecciones
        .iter()
        .find(|c| c.nombre == "Conflicto SOIP 1965-66")
        .expect("la colección «Conflicto SOIP 1965-66» debe estar en el corpus real");

    // Criterio de aceptación de Fase 0: 136 de 148 items sin procesar.
    assert_eq!(conflicto.items, 148);
    assert_eq!(conflicto.items_sin_procesar(), 136);
    assert_eq!(conflicto.chunks, 40);

    // La tabla de cobertura del informe lo declara explícitamente.
    let tabla = entropia_agent::informe::tabla_cobertura(&cobertura);
    assert!(tabla.contains("| Conflicto SOIP 1965-66 | 148 | 12 | 136 | 40 |"));
    // Totales del recorte real (verificados sobre el corpus 2026-08-05).
    assert_eq!(cobertura.items_total, 418);
    assert_eq!(cobertura.items_sin_procesar, 255);
    assert!(tabla.contains("| **Total** | **418** | **163** | **255** | — |"));
}

//! Test de integración contra el corpus real (`entropia.sqlite`), que
//! verifica los criterios de aceptación de Fase 0 (PLAN §9):
//!
//! - ninguna consulta devuelve chunks de colecciones excluidas;
//! - un informe sobre «Conflicto SOIP 1965-66» declara que 136 de 148 items no
//!   están procesados.
//!
//! Se salta si la base no está presente (p. ej. en CI sin el corpus).

mod common;
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

// ── El workflow completo sobre el corpus real (PLAN §9, Fases 5-6) ────────
//
// Los tests del motor corren sobre un fixture de cuatro chunks y modelos
// falsos: eso prueba la lógica, no que la máquina aguante el corpus. Acá hay
// 418 items reales, 61 % sin procesar, texto Markdown con HTML embebido y
// títulos que a veces son fechas y a veces «DSC00949». Este test corre el
// ciclo entero contra la base real, y se salta si no está.

use entropia_agent::estado::EstadoDb;
use entropia_agent::investigacion::procesar;
use serde_json::{json, Value};

fn dir_temporal(nombre: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "entropia-real-{nombre}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn coleccion_por_nombre(repo: &RepositorioSqlite, nombre: &str) -> Option<String> {
    repo.listar_todas_las_colecciones()
        .into_iter()
        .find(|c| c.nombre == nombre)
        .map(|c| c.id)
}

#[test]
fn el_workflow_completo_corre_sobre_el_corpus_real() {
    let Some(path) = ruta_corpus() else {
        eprintln!("sin corpus real: se salta el ciclo completo");
        return;
    };
    let repo = RepositorioSqlite::abrir(&path).unwrap();
    let Some(coleccion) = coleccion_por_nombre(&repo, "Conflicto SOIP 1965-66") else {
        eprintln!("el corpus no tiene «Conflicto SOIP 1965-66»: se salta");
        return;
    };
    // Estado y artefactos en temporales: la base real se abre solo lectura y
    // el `estado.sqlite` del usuario no se toca.
    let dir = dir_temporal("ciclo");
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let llm = common::LlmGuionado {
        consultas: vec![
            "huelga".into(),
            "gremio".into(),
            "trabajadores".into(),
            "sindicato".into(),
        ],
    };

    let creado = procesar(&db,&repo,&llm,None,&dir,json!({"op":"create","question":"¿Cómo se organizó el conflicto gremial?","project":"corpus-real","collection_ids":[coleccion.clone()],"max_llm_calls":150,"max_cost":50.0})).unwrap();
    let id = creado["job"]["id"].as_str().unwrap().to_owned();

    let mut out = creado;
    for _ in 0..150 {
        match out["job"]["status"].as_str() {
            Some("done") => break,
            Some("awaiting_human") => {
                let preguntas = out["artifacts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .rfind(|a| a["kind"] == "clarification_round" && a["obsolete"] == false)
                    .expect("la ronda tiene que existir")["content"]["questions"]
                    .as_array()
                    .unwrap()
                    .clone();
                let respuestas: Vec<Value> = preguntas
                    .iter()
                    .map(|q| json!({"id":q["id"],"text":"1965, conflicto gremial, actas y volantes"}))
                    .collect();
                out = procesar(
                    &db,
                    &repo,
                    &llm,
                    None,
                    &dir,
                    json!({"op":"answer","job_id":id,"answers":respuestas}),
                )
                .unwrap();
            }
            _ => {
                out = procesar(
                    &db,
                    &repo,
                    &llm,
                    None,
                    &dir,
                    json!({"op":"advance","job_id":id}),
                )
                .unwrap()
            }
        }
    }
    assert_eq!(out["job"]["status"], "done", "{}", out["job"]);

    let artefacto = |kind: &str| -> Value {
        out["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .rfind(|a| a["kind"] == kind && a["obsolete"] == false)
            .unwrap_or_else(|| panic!("falta el artefacto {kind}"))["content"]
            .clone()
    };

    // 1. La cobertura declara el recorte real: 148 items, 12 procesados.
    let cobertura = artefacto("coverage")["collections"][0].clone();
    assert_eq!(cobertura["items"], 148, "{cobertura}");
    assert_eq!(cobertura["items_with_chunks"], 12, "{cobertura}");

    // 2. Toda la evidencia sale del recorte elegido y trae identidad completa.
    let archive = artefacto("archive");
    let evidencia = archive["evidence"].as_array().unwrap().clone();
    assert!(!evidencia.is_empty(), "no se recuperó evidencia real");
    for e in &evidencia {
        assert_eq!(e["collection_id"], coleccion.as_str(), "{e}");
        assert!(e["item_id"].as_str().is_some_and(|s| !s.is_empty()), "{e}");
        assert!(e["text"].as_str().is_some_and(|s| !s.is_empty()), "{e}");
    }

    // 3. Cada pasaje citado es literal contra el texto de su fuente: sobre
    //    Markdown con HTML embebido y acentos, no sobre un fixture ASCII.
    let claims = archive["claims"].as_array().unwrap().clone();
    assert!(!claims.is_empty(), "el archivo no produjo claims");
    for claim in &claims {
        for q in claim["quotes"].as_array().unwrap() {
            let fuente = evidencia
                .iter()
                .find(|e| e["id"] == q["evidence_id"])
                .expect("la cita apunta a evidencia del lote");
            assert!(
                fuente["text"]
                    .as_str()
                    .unwrap()
                    .contains(q["quote"].as_str().unwrap()),
                "cita no literal: {q}"
            );
            assert!(q["span_start"].is_i64(), "la cita tiene que ubicarse: {q}");
        }
    }

    // 4. La precisión nunca sobrepasa lo que la fuente sostiene. Un título
    //    como «65-04-12-a» da precisión de día; uno como «DSC01129» no tiene
    //    fecha y cae a la capa de la colección: año, con su confianza y su
    //    derivación declaradas. Eso no es inventar, es inferir y decir de
    //    dónde salió.
    let mut por_dia = 0;
    for e in &evidencia {
        let titulo = e["title"].as_str().unwrap_or_default();
        let fecha = &e["document_date"];
        if fecha["iso"].is_null() {
            continue;
        }
        let precision = fecha["precision"].as_str().unwrap_or_default();
        let derivacion = fecha["source"].as_str().unwrap_or_default();
        assert!(
            !derivacion.is_empty(),
            "toda fecha declara su derivación: {e}"
        );
        if titulo.starts_with("DSC") {
            assert_eq!(
                precision, "year",
                "«{titulo}» no tiene fecha propia: no puede afirmarse un día"
            );
            assert_eq!(derivacion, "coleccion", "{e}");
        }
        if precision == "day" {
            por_dia += 1;
            assert!(
                titulo.starts_with("65-"),
                "solo un título fechado sostiene precisión de día: «{titulo}»"
            );
        }
    }
    assert!(por_dia > 0, "ningún título fechado del recorte se leyó");

    eprintln!(
        "corpus real: {} fragmentos, {por_dia} fechados por día, {} claims",
        evidencia.len(),
        claims.len()
    );

    // 5. El informe cierra con sus fuentes y sin números colgados.
    let md = std::fs::read_to_string(dir.join(&id).join("report.md")).unwrap();
    assert!(md.contains("## Cobertura del recorte consultado"));
    assert!(
        md.contains("| **Total** | **148** | **12** | **136** | — |"),
        "{md}"
    );
    assert!(md.contains("## Fuentes citadas"), "{md}");
    assert!(
        entropia_agent::informe_render::citas_sin_referencia(&md).is_empty(),
        "quedó un [n] sin referencia sobre el corpus real:\n{md}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn un_recorte_sin_material_procesado_no_arranca_sobre_el_corpus_real() {
    let Some(path) = ruta_corpus() else {
        eprintln!("sin corpus real: se salta el freno de cobertura");
        return;
    };
    let repo = RepositorioSqlite::abrir(&path).unwrap();
    // «SOIP 1965» tiene 57 items y ningún chunk procesado.
    let Some(coleccion) = coleccion_por_nombre(&repo, "SOIP 1965") else {
        eprintln!("el corpus no tiene «SOIP 1965»: se salta");
        return;
    };
    let dir = dir_temporal("freno");
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let llm = common::LlmGuionado {
        consultas: vec!["huelga".into()],
    };
    let resultado = procesar(
        &db,
        &repo,
        &llm,
        None,
        &dir,
        json!({"op":"create","question":"¿Qué pasó?","project":"corpus-real","collection_ids":[coleccion],"max_llm_calls":20}),
    );
    let error = resultado.expect_err("un recorte sin chunks no puede arrancar");
    assert!(error.contains("material procesado"), "{error}");
    let _ = std::fs::remove_dir_all(&dir);
}

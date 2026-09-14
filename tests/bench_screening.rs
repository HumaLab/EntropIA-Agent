//! Filtro léxico de una tanda de candidatas del EntropIA-Bench, contra el
//! corpus real (`entropia.sqlite`). No hace llamadas pagas: solo FTS5.
//!
//! El banco crece con preguntas que la búsqueda léxica NO resuelve sola. Este
//! corredor mide exactamente la métrica del bench (`buscar_fts5` sobre la
//! pregunta, a `RERANK_DEPTH`, con recall por grupos de evidencia) y declara
//! el veredicto de cada candidata. También verifica que los chunks e items
//! esperados existan en el corpus: un id inventado no llega al banco.
//!
//! `TANDA` elige el archivo (`bench/candidatas/tanda-<N>.json`); sin ella toma
//! la tanda de número más alto. Es `#[ignore]`:
//! `TANDA=3 cargo test --test bench_screening -- --ignored --nocapture`.

use entropia_agent::recuperacion::RERANK_DEPTH;
use entropia_agent::repositorio::RepositorioSqlite;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Tanda {
    tanda: u32,
    candidatas: Vec<Candidata>,
}

#[derive(Debug, Deserialize)]
struct Candidata {
    id: String,
    tipo: String,
    pregunta: String,
    #[serde(default)]
    items_esperados: Vec<String>,
    #[serde(default)]
    chunk_ids_esperados: Vec<String>,
    #[serde(default)]
    grupos_esperados: Vec<Vec<String>>,
}

impl Candidata {
    /// Grupos de evidencia efectivos, con la misma regla que el banco: los
    /// declarados o, si no hay, uno por chunk esperado.
    fn grupos(&self) -> Vec<Vec<String>> {
        if self.grupos_esperados.is_empty() {
            self.chunk_ids_esperados
                .iter()
                .map(|c| vec![c.clone()])
                .collect()
        } else {
            self.grupos_esperados.clone()
        }
    }
}

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

/// Tanda de número más alto en `bench/candidatas`, si hay alguna.
fn ultima_tanda() -> Option<u32> {
    let mut numeros: Vec<u32> = std::fs::read_dir("bench/candidatas")
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let nombre = e.file_name().into_string().ok()?;
            let resto = nombre.strip_prefix("tanda-")?.strip_suffix(".json")?;
            resto.parse::<u32>().ok()
        })
        .collect();
    numeros.sort_unstable();
    numeros.pop()
}

/// Posición desde 1 del primer chunk del grupo entre los léxicos, o `-`.
fn posicion(grupo: &[String], lexicos: &[String]) -> String {
    grupo
        .iter()
        .filter_map(|c| lexicos.iter().position(|l| l == c).map(|p| p + 1))
        .min()
        .map(|p| p.to_string())
        .unwrap_or_else(|| "-".into())
}

#[test]
#[ignore = "filtra una tanda de candidatas contra el corpus real"]
fn filtra_la_tanda_de_candidatas() {
    let Some(path) = ruta_corpus() else {
        eprintln!("sin corpus real: se salta el filtro");
        return;
    };
    let numero = match std::env::var("TANDA").ok().and_then(|t| t.parse().ok()) {
        Some(n) => n,
        None => match ultima_tanda() {
            Some(n) => n,
            None => {
                eprintln!("no hay tandas en bench/candidatas");
                return;
            }
        },
    };
    let archivo = format!("bench/candidatas/tanda-{numero}.json");
    let raw = std::fs::read_to_string(&archivo)
        .unwrap_or_else(|e| panic!("tanda ilegible ({archivo}): {e}"));
    let tanda: Tanda =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("tanda inválida ({archivo}): {e}"));
    let repo = RepositorioSqlite::abrir(&path).unwrap();
    let chunks_del_corpus: std::collections::HashSet<String> = repo
        .cargar_chunks()
        .unwrap()
        .into_iter()
        .map(|c| c.id)
        .collect();

    println!(
        "\nTanda {} — {} candidatas",
        tanda.tanda,
        tanda.candidatas.len()
    );
    println!("filtro: recall léxico sobre grupos de evidencia, k = {RERANK_DEPTH}\n");

    let mut pasan = 0;
    let mut problemas: Vec<String> = Vec::new();
    for c in &tanda.candidatas {
        let grupos = c.grupos();
        if grupos.is_empty() {
            problemas.push(format!("{}: no declara evidencia esperada", c.id));
            continue;
        }
        for chunk in &c.chunk_ids_esperados {
            if !chunks_del_corpus.contains(chunk) {
                problemas.push(format!("{}: el chunk {chunk} no está en el corpus", c.id));
            }
        }
        for item in &c.items_esperados {
            if !entropia_agent::bench::item_tiene_chunks(&repo, item) {
                problemas.push(format!("{}: el item {item} no tiene chunks", c.id));
            }
        }

        let lexicos = repo.buscar_fts5(&c.pregunta, RERANK_DEPTH);
        let cubiertos = grupos
            .iter()
            .filter(|g| g.iter().any(|c| lexicos.contains(c)))
            .count();
        let recall = cubiertos as f64 / grupos.len() as f64;
        let veredicto = if recall == 0.0 {
            pasan += 1;
            "PASA"
        } else {
            "LA RESUELVE LA LÉXICA"
        };
        let posiciones: Vec<String> = grupos.iter().map(|g| posicion(g, &lexicos)).collect();
        println!("{} [{}] {veredicto}", c.id, c.tipo);
        println!(
            "  recall léxico {recall:.2} ({cubiertos}/{}), posiciones: {}",
            grupos.len(),
            posiciones.join(", ")
        );
        println!("  {}\n", c.pregunta);
    }

    println!("pasan el filtro: {pasan} de {}", tanda.candidatas.len());
    if !problemas.is_empty() {
        println!("\nproblemas:");
        for p in &problemas {
            println!("  - {p}");
        }
    }
    assert!(problemas.is_empty(), "la tanda tiene evidencia inválida");
}

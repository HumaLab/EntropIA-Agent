//! Corredor del EntropIA-Bench contra el corpus real (`entropia.sqlite`), solo
//! sobre la recuperación: no corre investigaciones.
//!
//! Con `OPENROUTER_API_KEY` en el entorno mide la recuperación híbrida del
//! flujo (una llamada de embeddings y una de rerank por pregunta con chunks
//! esperados); sin ella, solo la línea base léxica, y el resultado lo declara.
//! La clave se lee solo del entorno, no de `.env`: una corrida léxica nunca
//! hace llamadas pagas por accidente.
//!
//! Escribe `bench/resultados/<AAAA-MM-DD>.json`. Es `#[ignore]`: se corre a
//! mano con `cargo test --test bench_real -- --ignored --nocapture`.

use entropia_agent::bench::{cargar_banco, correr_recuperacion, Agregado};
use entropia_agent::embeddings::ClienteEmbeddings;
use entropia_agent::recuperacion::Recuperador;
use entropia_agent::repositorio::RepositorioSqlite;
use entropia_agent::rerank::ClienteRerank;

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

/// `0.50 (1/12)`, o `n/a (0/12)` si la métrica no aplica en ninguna pregunta.
fn formatear(a: &Agregado) -> String {
    match a.media {
        Some(media) => format!("{media:.2} ({}/{})", a.aplicables, a.total),
        None => format!("n/a ({}/{})", a.aplicables, a.total),
    }
}

#[test]
#[ignore = "corre el banco contra el corpus real; con clave, hace llamadas pagas"]
fn corre_el_banco_sobre_la_recuperacion_real() {
    let Some(path) = ruta_corpus() else {
        eprintln!("sin corpus real: se salta el bench");
        return;
    };
    let repo = RepositorioSqlite::abrir(&path).unwrap();
    let banco = cargar_banco("bench/preguntas.json").unwrap();
    let recuperador = std::env::var("OPENROUTER_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .map(|key| Recuperador::new(ClienteEmbeddings::new(key.clone()), ClienteRerank::new(key)));

    let resultado = correr_recuperacion(&repo, recuperador.as_ref(), &banco);

    let hoy = time::OffsetDateTime::now_local()
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
        .date();
    let fecha = hoy
        .format(time::macros::format_description!("[year]-[month]-[day]"))
        .unwrap();
    let dir = std::path::Path::new("bench/resultados");
    std::fs::create_dir_all(dir).unwrap();
    let salida = dir.join(format!("{fecha}.json"));
    std::fs::write(&salida, serde_json::to_string_pretty(&resultado).unwrap()).unwrap();

    let r = &resultado.resumen;
    println!(
        "{} [{}, k={}]: retrieval_recall {} · léxico {} · cobertura {} → {}",
        resultado.banco,
        resultado.pipeline,
        resultado.k,
        formatear(&r.retrieval_recall),
        formatear(&r.retrieval_recall_lexico),
        formatear(&r.cobertura_items_esperados),
        salida.display()
    );
}

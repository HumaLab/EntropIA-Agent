//! Captura de las piernas de producción de la recuperación contra el corpus
//! real (`entropia.sqlite`), para evaluar fusiones RRF ponderadas fuera de
//! línea con `bench_fusion` sin volver a pagar.
//!
//! Requiere `OPENROUTER_API_KEY` en el entorno: una llamada de embeddings por
//! pregunta con evidencia esperada, ninguna de rerank; las preguntas sin
//! evidencia no se consultan. Como en `bench_real`, la clave se lee solo del
//! entorno, no de `.env`.
//!
//! Las piernas son las del flujo: la vectorial cortada en `LEG_K` y la léxica
//! armada como en `recuperar_en_colecciones` (los primeros `LEG_K * 8`
//! resultados de FTS5, filtrados al alcance y cortados en `LEG_K`).
//!
//! Escribe `bench/resultados/<AAAA-MM-DD>-piernas.json`. Es `#[ignore]`:
//! `cargo test --test bench_piernas -- --ignored --nocapture`.

use entropia_agent::bench::{
    alcance_seleccionar_todo, capturar_piernas, cargar_banco, ArchivoPiernas, PiernasPregunta,
};
use entropia_agent::embeddings::ClienteEmbeddings;
use entropia_agent::recuperacion::{Recuperador, LEG_K, RRF_K};
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

#[test]
#[ignore = "captura las piernas contra el corpus real; hace llamadas pagas de embeddings"]
fn captura_las_piernas_de_la_recuperacion_real() {
    let Some(path) = ruta_corpus() else {
        eprintln!("sin corpus real: se salta la captura de piernas");
        return;
    };
    let Some(key) = std::env::var("OPENROUTER_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
    else {
        eprintln!(
            "sin OPENROUTER_API_KEY: sin pierna semántica no hay nada que ponderar; se salta"
        );
        return;
    };
    let repo = RepositorioSqlite::abrir(&path).unwrap();
    let banco = cargar_banco("bench/preguntas.json").unwrap();
    let recuperador =
        Recuperador::new(ClienteEmbeddings::new(key.clone()), ClienteRerank::new(key));

    // Sin evidencia esperada `capturar_piernas` no consulta: no se paga una
    // llamada por una pregunta que no se puede evaluar.
    let piernas: Vec<PiernasPregunta> = banco
        .preguntas
        .iter()
        .filter_map(|p| capturar_piernas(&repo, &recuperador, p))
        .collect();
    let archivo = ArchivoPiernas {
        banco: banco.banco.clone(),
        leg_k: LEG_K,
        rrf_k: RRF_K,
        alcance: alcance_seleccionar_todo(&repo),
        piernas,
    };

    let hoy = time::OffsetDateTime::now_local()
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
        .date();
    let fecha = hoy
        .format(time::macros::format_description!("[year]-[month]-[day]"))
        .unwrap();
    let dir = std::path::Path::new("bench/resultados");
    std::fs::create_dir_all(dir).unwrap();
    let salida = dir.join(format!("{fecha}-piernas.json"));
    std::fs::write(&salida, serde_json::to_string_pretty(&archivo).unwrap()).unwrap();

    for p in &archivo.piernas {
        let mut linea = format!(
            "{}  vectorial={} léxica={}",
            p.pregunta_id,
            p.vectorial.len(),
            p.lexica.len()
        );
        if let Some(motivo) = &p.degradacion {
            linea.push_str(&format!("  [degradado: {motivo}]"));
        }
        println!("{linea}");
    }
    println!(
        "{} preguntas con evidencia de {} (LEG_K={LEG_K}, RRF_K={RRF_K}) → {}",
        archivo.piernas.len(),
        banco.preguntas.len(),
        salida.display()
    );
}

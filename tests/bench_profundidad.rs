//! Diagnóstico de profundidad de la recuperación contra el corpus real
//! (`entropia.sqlite`): en qué posición de cada pierna y de la fusión RRF
//! aparece la evidencia esperada, para calibrar `RERANK_DEPTH`. No reranquea:
//! el rerank solo reordena el top de la fusión y no rescata lo que no llegó.
//!
//! Requiere `OPENROUTER_API_KEY` en el entorno (una llamada de embeddings por
//! pregunta con evidencia, ninguna de rerank): sin pierna semántica el
//! diagnóstico no dice nada del flujo real. Como en `bench_real`, la clave se
//! lee solo del entorno, no de `.env`.
//!
//! `BENCH_IDS` (opcional) acota a ids separados por coma; por defecto entra
//! toda pregunta con evidencia esperada.
//!
//! Cómo leer los `k` (con `LEG_K` = 24 por pierna y `RERANK_DEPTH` = 16):
//! - Las posiciones por pierna (`v`, `l`) son exactas hasta 48: hasta 24 están
//!   dentro de lo que el flujo trae; más allá simulan subir `LEG_K`.
//! - `f` y la curva principal (`recall_fusion`) salen de la fusión del flujo:
//!   piernas de 24, el pool de candidatos real (a lo sumo 48). Sus primeros 16
//!   son los que van al rerank.
//! - La curva secundaria (`recall_fusion_leg_k_simulado`) fusiona las piernas
//!   medidas a 48: simula subir `LEG_K` a 48. No es la del flujo aun en sus
//!   primeras posiciones: un chunk hondo en las dos piernas puede sumar más que
//!   uno alto en una sola.
//! - Los empates de RRF se desempatan por la mejor posición en alguna pierna y
//!   después por id: dos corridas dan el mismo orden.
//!
//! Escribe `bench/resultados/<AAAA-MM-DD>-profundidad.json`. Es `#[ignore]`:
//! `BENCH_IDS=soip-001,soip-002 cargo test --test bench_profundidad -- --ignored --nocapture`.

use entropia_agent::bench::{
    alcance_seleccionar_todo, cargar_banco, diagnosticar_profundidad, resumir_profundidad,
    Agregado, DiagnosticoProfundidad, PreguntaBench, RecallMedioEnK,
};
use entropia_agent::embeddings::ClienteEmbeddings;
use entropia_agent::recuperacion::{Recuperador, LEG_K, RERANK_DEPTH};
use entropia_agent::repositorio::RepositorioSqlite;
use entropia_agent::rerank::ClienteRerank;

/// Profundidades de la curva de recall de la fusión.
const KS: [usize; 5] = [8, 16, 24, 32, 48];

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

/// Posición desde 1, o `-` si no aparece dentro de la profundidad.
fn posicion(p: Option<usize>) -> String {
    match p {
        Some(p) => p.to_string(),
        None => "-".into(),
    }
}

/// `0.50`, o `n/a` si la pregunta no declara evidencia.
fn recall(r: Option<f64>) -> String {
    match r {
        Some(r) => format!("{r:.2}"),
        None => "n/a".into(),
    }
}

/// `0.50 (1/12)`, o `n/a (0/12)` si la métrica no aplica en ninguna pregunta.
fn formatear(a: &Agregado) -> String {
    match a.media {
        Some(media) => format!("{media:.2} ({}/{})", a.aplicables, a.total),
        None => format!("n/a ({}/{})", a.aplicables, a.total),
    }
}

/// Una línea por pregunta: posiciones de cada grupo y curva de recall.
fn linea(d: &DiagnosticoProfundidad) -> String {
    let grupos: Vec<String> = d
        .grupos
        .iter()
        .map(|g| {
            format!(
                "v={} l={} f={}",
                posicion(g.vectorial),
                posicion(g.lexica),
                posicion(g.fusion)
            )
        })
        .collect();
    let curva: Vec<String> = d
        .recall_fusion
        .iter()
        .map(|r| format!("@{}={}", r.k, recall(r.recall)))
        .collect();
    let mut s = format!(
        "{}  {}  recall_fusion {}",
        d.pregunta_id,
        grupos.join(" | "),
        curva.join(" ")
    );
    if let Some(motivo) = &d.degradacion {
        s.push_str(&format!("  [degradado: {motivo}]"));
    }
    s
}

#[test]
#[ignore = "diagnostica la profundidad contra el corpus real; hace llamadas pagas de embeddings"]
fn diagnostica_la_profundidad_de_la_recuperacion_real() {
    let Some(path) = ruta_corpus() else {
        eprintln!("sin corpus real: se salta el diagnóstico de profundidad");
        return;
    };
    let Some(key) = std::env::var("OPENROUTER_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
    else {
        eprintln!(
            "sin OPENROUTER_API_KEY: sin pierna semántica el diagnóstico no mide el flujo real; \
             se salta"
        );
        return;
    };
    let repo = RepositorioSqlite::abrir(&path).unwrap();
    let banco = cargar_banco("bench/preguntas.json").unwrap();
    let recuperador =
        Recuperador::new(ClienteEmbeddings::new(key.clone()), ClienteRerank::new(key));

    let pedidos: Vec<String> = std::env::var("BENCH_IDS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    for id in &pedidos {
        if !banco.preguntas.iter().any(|p| &p.id == id) {
            eprintln!("BENCH_IDS: «{id}» no está en el banco");
        }
    }
    let preguntas: Vec<&PreguntaBench> = if pedidos.is_empty() {
        banco
            .preguntas
            .iter()
            .filter(|p| !p.chunk_ids_esperados.is_empty())
            .collect()
    } else {
        banco
            .preguntas
            .iter()
            .filter(|p| pedidos.contains(&p.id))
            .collect()
    };

    let diagnosticos: Vec<DiagnosticoProfundidad> = preguntas
        .iter()
        .map(|p| diagnosticar_profundidad(&repo, &recuperador, p, &KS))
        .collect();
    let resumen = resumir_profundidad(&diagnosticos, &KS);

    let profundidad = KS.iter().copied().max().unwrap_or(0);
    let resultado = serde_json::json!({
        "banco": banco.banco,
        "pipeline": "hibrido_sin_rerank",
        "ks": KS,
        "leg_k": LEG_K,
        "rerank_depth": RERANK_DEPTH,
        "nota": format!(
            "posiciones por pierna exactas: hasta LEG_K={LEG_K} están dentro del flujo. \
             f y recall_fusion salen de la fusión del flujo (piernas de LEG_K, el pool \
             real); recall_fusion_leg_k_simulado fusiona piernas de {profundidad} y simula \
             subir LEG_K. Sin rerank."
        ),
        "alcance": alcance_seleccionar_todo(&repo),
        "diagnosticos": diagnosticos,
        "resumen": resumen,
    });

    let hoy = time::OffsetDateTime::now_local()
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
        .date();
    let fecha = hoy
        .format(time::macros::format_description!("[year]-[month]-[day]"))
        .unwrap();
    let dir = std::path::Path::new("bench/resultados");
    std::fs::create_dir_all(dir).unwrap();
    let salida = dir.join(format!("{fecha}-profundidad.json"));
    std::fs::write(&salida, serde_json::to_string_pretty(&resultado).unwrap()).unwrap();

    for d in &diagnosticos {
        println!("{}", linea(d));
    }
    let curva = |puntos: &[RecallMedioEnK]| -> String {
        puntos
            .iter()
            .map(|r| format!("@{}={}", r.k, formatear(&r.recall)))
            .collect::<Vec<_>>()
            .join(" ")
    };
    println!(
        "recall_fusion medio (flujo, LEG_K={LEG_K}): {}",
        curva(&resumen.recall_fusion)
    );
    println!(
        "recall_fusion medio (LEG_K simulado en {profundidad}): {}",
        curva(&resumen.recall_fusion_leg_k_simulado)
    );
    println!("→ {}", salida.display());
}

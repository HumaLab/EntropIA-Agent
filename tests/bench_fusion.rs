//! Evaluación fuera de línea de la fusión RRF ponderada sobre las piernas de
//! producción que captura `bench_piernas`: ¿pesar más la pierna vectorial
//! recupera evidencia que la fusión pareja empuja hacia abajo, sin subir la
//! profundidad? No llama a ninguna API ni abre el corpus: lee el banco y el
//! archivo de piernas.
//!
//! `BENCH_PIERNAS` (opcional) apunta al archivo de piernas; por defecto se usa
//! el `*-piernas.json` más reciente de `bench/resultados/`.
//!
//! Escribe `bench/resultados/<AAAA-MM-DD>-fusion.json`. Es `#[ignore]`:
//! `cargo test --test bench_fusion -- --ignored --nocapture`.

use std::path::PathBuf;

use entropia_agent::bench::{
    cargar_banco, evaluar_fusiones, Agregado, ArchivoPiernas, EvaluacionFusion, Subconjunto,
};
use entropia_agent::recuperacion::{LEG_K, RERANK_DEPTH, RRF_K};

/// Pares `(vectorial, léxico)`. El primero es la fusión de producción: contra
/// él se comparan los demás. `(1, 0)` es solo vectorial y `(0, 1)` solo léxica.
const PESOS: [(f64, f64); 6] = [
    (1.0, 1.0),
    (1.5, 1.0),
    (2.0, 1.0),
    (3.0, 1.0),
    (1.0, 0.0),
    (0.0, 1.0),
];

/// `RERANK_DEPTH` es la profundidad que va al rerank; 8 y 24 dan contexto.
const KS: [usize; 3] = [8, RERANK_DEPTH, 24];

/// Fáciles: las 22 preguntas originales del banco. Difíciles: las que se
/// agregaron después para las fallas de recuperación.
fn subconjuntos() -> Vec<Subconjunto> {
    let ids = |desde: u32, hasta: u32| {
        (desde..=hasta)
            .map(|n| format!("soip-{n:03}"))
            .collect::<Vec<_>>()
    };
    vec![
        Subconjunto {
            nombre: "fáciles".into(),
            preguntas: ids(1, 22),
        },
        Subconjunto {
            nombre: "difíciles".into(),
            preguntas: ids(23, 32),
        },
    ]
}

/// `BENCH_PIERNAS`, o el `*-piernas.json` más reciente de `bench/resultados/`.
fn archivo_de_piernas() -> Option<PathBuf> {
    if let Ok(ruta) = std::env::var("BENCH_PIERNAS") {
        if !ruta.trim().is_empty() {
            return Some(PathBuf::from(ruta));
        }
    }
    let mut candidatos: Vec<PathBuf> = std::fs::read_dir("bench/resultados")
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with("-piernas.json"))
        })
        .collect();
    // El nombre empieza con la fecha AAAA-MM-DD: el mayor es el más reciente.
    candidatos.sort();
    candidatos.pop()
}

/// `(2,1)`, `(1.5,1)`.
fn nombre_pesos((vectorial, lexico): (f64, f64)) -> String {
    format!("({vectorial},{lexico})")
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

#[test]
#[ignore = "evalúa fusiones ponderadas sobre piernas ya capturadas; no hace llamadas"]
fn evalua_la_fusion_ponderada_sobre_las_piernas_capturadas() {
    let Some(ruta) = archivo_de_piernas() else {
        eprintln!(
            "sin archivo de piernas (BENCH_PIERNAS o bench/resultados/*-piernas.json): \
             primero hay que correr bench_piernas; se salta"
        );
        return;
    };
    let raw = std::fs::read_to_string(&ruta)
        .unwrap_or_else(|e| panic!("{} ilegible: {e}", ruta.display()));
    let archivo: ArchivoPiernas =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{} inválido: {e}", ruta.display()));
    let banco = cargar_banco("bench/preguntas.json").unwrap();
    if archivo.banco != banco.banco {
        eprintln!(
            "aviso: las piernas son del banco «{}» y el banco actual es «{}»",
            archivo.banco, banco.banco
        );
    }
    if archivo.leg_k != LEG_K || archivo.rrf_k != RRF_K {
        eprintln!(
            "aviso: piernas capturadas con LEG_K={} y RRF_K={}; el flujo actual usa \
             LEG_K={LEG_K} y RRF_K={RRF_K}",
            archivo.leg_k, archivo.rrf_k
        );
    }
    let degradadas: Vec<&str> = archivo
        .piernas
        .iter()
        .filter(|p| p.degradacion.is_some())
        .map(|p| p.pregunta_id.as_str())
        .collect();
    if !degradadas.is_empty() {
        eprintln!(
            "aviso: sin pierna semántica en {}: pesarla no las mueve",
            degradadas.join(", ")
        );
    }

    let subconjuntos = subconjuntos();
    let evaluaciones: Vec<EvaluacionFusion> = KS
        .iter()
        .map(|&k| evaluar_fusiones(&banco, &archivo.piernas, &PESOS, &subconjuntos, k))
        .collect();

    let resultado = serde_json::json!({
        "banco": banco.banco,
        "piernas": ruta.display().to_string(),
        "leg_k": archivo.leg_k,
        "rrf_k": RRF_K,
        "alcance": archivo.alcance,
        "pesos": PESOS,
        "subconjuntos": subconjuntos,
        "nota": "recall de grupos de evidencia en los primeros k de la fusión RRF \
                 ponderada, sin rerank; pesos (vectorial, léxico), el primero es el \
                 de producción",
        "evaluaciones": evaluaciones,
    });
    let hoy = time::OffsetDateTime::now_local()
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
        .date();
    let fecha = hoy
        .format(time::macros::format_description!("[year]-[month]-[day]"))
        .unwrap();
    let dir = std::path::Path::new("bench/resultados");
    std::fs::create_dir_all(dir).unwrap();
    let salida = dir.join(format!("{fecha}-fusion.json"));
    std::fs::write(&salida, serde_json::to_string_pretty(&resultado).unwrap()).unwrap();

    println!("piernas: {}", ruta.display());
    for e in &evaluaciones {
        println!("k={}", e.k);
        for f in &e.pesos {
            let por_subconjunto: Vec<String> = f
                .subconjuntos
                .iter()
                .map(|s| format!("{} {}", s.nombre, formatear(&s.recall)))
                .collect();
            println!(
                "  {:<8} todas {}  {}",
                nombre_pesos((f.peso_vectorial, f.peso_lexico)),
                formatear(&f.recall),
                por_subconjunto.join("  ")
            );
        }
    }

    let base = nombre_pesos(PESOS[0]);
    println!("preguntas cuyo recall cambia frente a {base} en k={RERANK_DEPTH}:");
    let en_rerank = evaluaciones
        .iter()
        .find(|e| e.k == RERANK_DEPTH)
        .expect("KS incluye RERANK_DEPTH");
    for p in &en_rerank.preguntas {
        let referencia = p.recall[0];
        let cambios: Vec<String> = PESOS
            .iter()
            .zip(&p.recall)
            .skip(1)
            .filter(|(_, r)| **r != referencia)
            .map(|(&pesos, &r)| format!("{}={}", nombre_pesos(pesos), recall(r)))
            .collect();
        if !cambios.is_empty() {
            println!(
                "  {}  {base}={}  {}",
                p.pregunta_id,
                recall(referencia),
                cambios.join(" ")
            );
        }
    }
    println!("→ {}", salida.display());
}

//! Render del artefacto `report` a Markdown.
//!
//! El informe se entrega con el formato del agente original: abre con la tabla
//! de cobertura del recorte, reproduce los fragmentos de las fuentes usadas
//! entre comillas con su número de cita, y cierra con «## Fuentes citadas»,
//! una línea por cada `[n]` con colección, título e id del fragmento.
//!
//! Este módulo no decide qué se cita: recibe el artefacto ya ensamblado por
//! `investigacion::sanitize_report`, donde la numeración y los fragmentos son
//! deterministas. Acá solo se maqueta.

use serde_json::Value;
use std::collections::BTreeSet;

/// Renderiza el artefacto `report` completo a Markdown.
pub fn render(artifact: &Value) -> String {
    let report = &artifact["report"];
    let mut out = String::new();

    let titulo = report["title"].as_str().unwrap_or("Informe");
    out.push_str(&format!("# {titulo}\n\n"));

    out.push_str(&tabla_cobertura(&artifact["coverage"]));
    out.push_str(&encabezado_perfil(&artifact["profile"]));
    out.push_str(&busquedas_corpus(&artifact["retrieval_calls"]));
    out.push_str(&advertencia_cobertura(&artifact["coverage_warning"]));
    out.push_str(&encuadre(&artifact["clarification"]));

    for section in report["sections"].as_array().into_iter().flatten() {
        let titulo = section["title"].as_str().unwrap_or("Sección");
        out.push_str(&format!("## {titulo}\n\n"));
        // El texto que escribió el historiador no pasó por la verificación:
        // se declara, y sus corchetes se neutralizan como los del encuadre.
        let editada = section["origen"] == "historiador";
        if editada {
            out.push_str(
                "*Sección editada por el historiador: el texto no pasó por la verificación.*\n\n",
            );
        }
        let texto = section["text"].as_str().unwrap_or("").trim();
        if !texto.is_empty() {
            if editada {
                out.push_str(&sin_corchetes(texto));
            } else {
                out.push_str(texto);
            }
            out.push_str("\n\n");
        }
        for quote in section["quotes"].as_array().into_iter().flatten() {
            out.push_str(&bloque_cita(quote));
        }
    }

    out.push_str(&limitaciones(artifact));
    out.push_str(&fuentes_citadas(&report["references"]));
    out
}

/// Tabla de cobertura del recorte consultado.
///
/// Un informe que no declara lo que no pudo leer es engañoso, aunque cada
/// afirmación esté verificada.
fn tabla_cobertura(coverage: &Value) -> String {
    let colecciones = match coverage["collections"].as_array() {
        Some(c) if !c.is_empty() => c,
        _ => return String::new(),
    };
    let mut out = String::from("## Cobertura del recorte consultado\n\n");
    out.push_str("| Colección | Items | Con chunks | Sin procesar | Chunks |\n");
    out.push_str("|---|---|---|---|---|\n");
    let (mut items, mut con_chunks) = (0_i64, 0_i64);
    for c in colecciones {
        let i = c["items"].as_i64().unwrap_or(0);
        let cc = c["items_with_chunks"].as_i64().unwrap_or(0);
        items += i;
        con_chunks += cc;
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            celda(c["name"].as_str().unwrap_or("sin nombre")),
            i,
            cc,
            i - cc,
            c["chunks"].as_i64().unwrap_or(0)
        ));
    }
    out.push_str(&format!(
        "| **Total** | **{items}** | **{con_chunks}** | **{}** | — |\n\n",
        items - con_chunks
    ));
    out
}

/// El perfil sesga qué se recupera y qué se extrae. Se declara el instrumento
/// como se declara el recorte.
fn encabezado_perfil(profile: &Value) -> String {
    let nombre = match profile["name"].as_str() {
        Some(n) if !n.trim().is_empty() => n,
        _ => return String::new(),
    };
    let mut out = format!("**Perfil de informe:** {nombre}\n");
    if let Some(sesgo) = profile["bias"].as_str().filter(|s| !s.trim().is_empty()) {
        out.push_str(&format!("**Sesgo declarado del perfil:** {sesgo}\n"));
    }
    out.push('\n');
    out
}

/// Búsquedas que la recuperación hizo en el corpus y las llamadas externas que
/// costaron. No salen del presupuesto de llamadas al modelo, así que se
/// declaran junto a la cobertura: sin esta línea serían un gasto invisible.
fn busquedas_corpus(llamadas: &Value) -> String {
    let Some(consultas) = llamadas["queries"].as_u64() else {
        return String::new();
    };
    format!(
        "Búsquedas en el corpus: {consultas} ({} llamadas de embeddings, {} de rerank; no se descuentan del presupuesto de llamadas al modelo).\n\n",
        llamadas["embeddings"].as_u64().unwrap_or(0),
        llamadas["rerank"].as_u64().unwrap_or(0)
    )
}

fn advertencia_cobertura(warning: &Value) -> String {
    if warning["sufficient"].as_bool() != Some(false) {
        return String::new();
    }
    let mut out = String::from("> **Advertencia de cobertura.** ");
    out.push_str(warning["rationale"].as_str().unwrap_or(
        "La prospección no consideró suficiente el material disponible para la pregunta.",
    ));
    out.push('\n');
    for gap in warning["gaps"].as_array().into_iter().flatten() {
        if let Some(g) = gap.as_str().filter(|g| !g.trim().is_empty()) {
            out.push_str(&format!("> - {g}\n"));
        }
    }
    out.push('\n');
    out
}

/// Lo que el investigador respondió antes de que se armara el informe. Queda
/// en el documento porque delimita el alcance tanto como el recorte.
fn encuadre(clarification: &Value) -> String {
    let preguntas = match clarification["questions"].as_array() {
        Some(q) if !q.is_empty() => q,
        _ => return String::new(),
    };
    let respuestas = clarification["answers"].as_array();
    let mut out = String::from("## Encuadre acordado con el investigador\n\n");
    for pregunta in preguntas {
        let id = pregunta["id"].as_str().unwrap_or("");
        let texto = pregunta["text"].as_str().unwrap_or("");
        let respuesta = respuestas
            .into_iter()
            .flatten()
            .find(|a| a["id"] == *id)
            .and_then(|a| a["text"].as_str())
            .filter(|t| !t.trim().is_empty())
            .unwrap_or("*sin respuesta del investigador*");
        out.push_str(&format!(
            "- **{}** {}\n",
            sin_corchetes(texto),
            sin_corchetes(respuesta)
        ));
    }
    out.push('\n');
    out
}

/// El fragmento se reproduce literal; el prefijo `> ` es maquetado, no texto.
fn bloque_cita(quote: &Value) -> String {
    let n = quote["n"].as_i64().unwrap_or(0);
    let texto = quote["text"].as_str().unwrap_or("");
    let mut out = String::new();
    for linea in texto.lines() {
        out.push_str("> ");
        out.push_str(linea);
        out.push('\n');
    }
    if texto.is_empty() {
        out.push_str(">\n");
    }
    // El texto guardado es literal; la elipsis es del maquetado y avisa que el
    // fragmento sigue más allá de la ventana de cita.
    if quote["truncated"].as_bool() == Some(true) {
        out.push_str("> […]\n");
    }
    out.push_str(&format!("> \n> — [{n}] {}\n\n", pie_referencia(quote)));
    out
}

/// Colección · título · fragmento · rango de caracteres.
fn pie_referencia(r: &Value) -> String {
    let mut partes = Vec::new();
    if let Some(c) = r["collection"].as_str().filter(|c| !c.trim().is_empty()) {
        partes.push(c.to_string());
    }
    partes.push(
        r["title"]
            .as_str()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or("item sin título")
            .to_string(),
    );
    // La fecha del documento se declara con su precisión: un «1965» derivado
    // del año de la colección no puede leerse como si fuera un día exacto.
    if let Some(fecha) = r["date"].as_str().filter(|f| !f.trim().is_empty()) {
        partes.push(match r["date_precision"].as_str() {
            Some("month") => format!("{fecha} (mes)"),
            Some("year") => format!("{fecha} (año)"),
            _ => fecha.to_string(),
        });
    }
    partes.push(format!(
        "fragmento {}",
        r["chunk_id"].as_str().unwrap_or("sin id")
    ));
    if let (Some(a), Some(b)) = (r["start"].as_i64(), r["end"].as_i64()) {
        partes.push(format!("chars {a}–{b}"));
    }
    partes.join(" · ")
}

fn limitaciones(artifact: &Value) -> String {
    let mut lineas: Vec<String> = Vec::new();
    for l in artifact["archive_limitations"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if let Some(t) = l["text"].as_str().filter(|t| !t.trim().is_empty()) {
            match l["reason"].as_str().filter(|r| !r.trim().is_empty()) {
                Some(r) => lineas.push(format!("{t} ({r})")),
                None => lineas.push(t.to_string()),
            }
        }
    }
    let descartados = artifact["dropped_claims"]
        .as_array()
        .map(|d| d.len())
        .unwrap_or(0);
    match descartados {
        0 => {}
        1 => lineas.push(
            "1 afirmación se descartó por referenciar evidencia no suministrada o por venir incompleta."
                .to_string(),
        ),
        n => lineas.push(format!(
            "{n} afirmaciones se descartaron por referenciar evidencia no suministrada o por venir incompletas."
        )),
    }
    // Un rol que devolvió algo inusable y fue reemplazado por un artefacto
    // mínimo. El informe sale igual de prolijo: si no se declara, el
    // investigador no tiene cómo saberlo.
    for w in artifact["role_warnings"].as_array().into_iter().flatten() {
        let Some(error) = w["error"].as_str().filter(|e| !e.trim().is_empty()) else {
            continue;
        };
        let rol = w["role"].as_str().unwrap_or("rol desconocido");
        let veces = match w["times"].as_i64().unwrap_or(1) {
            n if n > 1 => format!(", {n} veces"),
            _ => String::new(),
        };
        lineas.push(format!(
            "**Degradación del pipeline:** {error} ({rol}{veces})."
        ));
    }
    if lineas.is_empty() {
        return String::new();
    }
    let mut out = String::from("## Limitaciones\n\n");
    for l in lineas {
        out.push_str(&format!("- {l}\n"));
    }
    out.push('\n');
    out
}

fn fuentes_citadas(references: &Value) -> String {
    let refs = match references.as_array() {
        Some(r) if !r.is_empty() => r,
        _ => return String::new(),
    };
    let mut out = String::from("## Fuentes citadas\n\n");
    for r in refs {
        out.push_str(&format!(
            "[{}] {}\n",
            r["n"].as_i64().unwrap_or(0),
            pie_referencia(r)
        ));
    }
    out.push('\n');
    out
}

fn celda(texto: &str) -> String {
    texto.replace('|', "\\|")
}

/// Neutraliza los corchetes del texto que escribe el investigador.
///
/// El guardrail de citas busca `[n]` en todo el cuerpo del informe: si una
/// respuesta del encuadre trae un `[3]` suelto, sin escapar quedaría contado
/// como una cita sin referencia. Escapado se lee igual y no falsifica nada.
fn sin_corchetes(texto: &str) -> String {
    texto.replace('[', "\\[").replace(']', "\\]")
}

/// Números citados en el cuerpo que no tienen línea propia en «Fuentes
/// citadas». Es el guardrail determinista heredado del agente original: nunca
/// se entrega un informe con un `[n]` colgado.
///
/// Por construcción devuelve un conjunto vacío; el test lo verifica sobre el
/// Markdown ya renderizado, que es lo que efectivamente lee el investigador.
pub fn citas_sin_referencia(markdown: &str) -> BTreeSet<i64> {
    let (cuerpo, cierre) = match markdown.split_once("## Fuentes citadas") {
        Some((c, f)) => (c, f),
        None => (markdown, ""),
    };
    let declaradas: BTreeSet<i64> = cierre
        .lines()
        .filter_map(|l| numero_inicial(l.trim()))
        .collect();
    numeros_citados(cuerpo)
        .difference(&declaradas)
        .copied()
        .collect()
}

/// `[12] ...` al inicio de una línea de referencia.
fn numero_inicial(linea: &str) -> Option<i64> {
    let resto = linea.strip_prefix('[')?;
    let (numero, _) = resto.split_once(']')?;
    numero.parse().ok()
}

fn numeros_citados(texto: &str) -> BTreeSet<i64> {
    let mut out = BTreeSet::new();
    let bytes = texto.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            let inicio = i + 1;
            let mut fin = inicio;
            while fin < bytes.len() && bytes[fin].is_ascii_digit() {
                fin += 1;
            }
            if fin > inicio && bytes.get(fin) == Some(&b']') {
                if let Ok(n) = texto[inicio..fin].parse() {
                    out.insert(n);
                }
                i = fin + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn artefacto() -> Value {
        json!({
            "report": {
                "title": "La huelga del SOIP",
                "sections": [{
                    "title": "Hechos",
                    "text": "El gremio paró en octubre.",
                    "claim_ids": ["c1"],
                    "quotes": [{
                        "n": 1,
                        "evidence_id": "chunk-1",
                        "chunk_id": "chunk-1",
                        "collection": "Conflicto SOIP 1965-66",
                        "title": "Acta de asamblea",
                        "text": "se resolvió la huelga general",
                        "start": 0,
                        "end": 29
                    }]
                }],
                "references": [{
                    "n": 1,
                    "evidence_id": "chunk-1",
                    "chunk_id": "chunk-1",
                    "collection": "Conflicto SOIP 1965-66",
                    "title": "Acta de asamblea",
                    "start": 0,
                    "end": 29
                }]
            },
            "coverage": {"collections": [
                {"id":"c1","name":"Conflicto SOIP 1965-66","items":148,"items_with_chunks":12,"chunks":40}
            ]},
            "coverage_warning": {"sufficient": true, "rationale": "", "gaps": []},
            "archive_limitations": [{"text": "1967 sin cobertura"}],
            "dropped_claims": [],
            "profile": {"id":"cronologia","name":"Cronología y crónica de eventos y procesos","bias":"Prioriza evidencia con anclaje temporal."},
            "clarification": {
                "questions": [{"id":"q1","text":"¿Con qué hecho abre la cronología?"}],
                "answers": [{"id":"q1","text":"Con la asamblea de octubre."}]
            }
        })
    }

    #[test]
    fn el_informe_abre_con_la_cobertura_y_cierra_con_las_fuentes_citadas() {
        let md = render(&artefacto());
        let cobertura = md.find("## Cobertura del recorte consultado").unwrap();
        let fuentes = md.find("## Fuentes citadas").unwrap();
        assert!(cobertura < fuentes);
        assert!(md.starts_with("# La huelga del SOIP"));
        assert!(md.contains("| **Total** | **148** | **12** | **136** | — |"));
    }

    #[test]
    fn el_fragmento_se_reproduce_literal_con_su_referencia() {
        let md = render(&artefacto());
        assert!(md.contains("> se resolvió la huelga general"));
        assert!(md.contains(
            "— [1] Conflicto SOIP 1965-66 · Acta de asamblea · fragmento chunk-1 · chars 0–29"
        ));
        assert!(md.contains("[1] Conflicto SOIP 1965-66 · Acta de asamblea · fragmento chunk-1"));
    }

    #[test]
    fn ninguna_cita_queda_sin_referencia() {
        assert!(citas_sin_referencia(&render(&artefacto())).is_empty());
    }

    #[test]
    fn el_guardrail_detecta_un_numero_citado_sin_linea_de_referencia() {
        let mut a = artefacto();
        a["report"]["references"] = json!([]);
        let colgadas = citas_sin_referencia(&render(&a));
        assert_eq!(colgadas.into_iter().collect::<Vec<_>>(), vec![1]);
    }

    #[test]
    fn el_perfil_y_su_sesgo_quedan_declarados_en_el_informe() {
        let md = render(&artefacto());
        assert!(md.contains("**Perfil de informe:** Cronología y crónica de eventos y procesos"));
        assert!(
            md.contains("**Sesgo declarado del perfil:** Prioriza evidencia con anclaje temporal.")
        );
    }

    #[test]
    fn el_encuadre_conserva_pregunta_y_respuesta() {
        let md = render(&artefacto());
        assert!(md.contains("- **¿Con qué hecho abre la cronología?** Con la asamblea de octubre."));
    }

    #[test]
    fn una_pregunta_sin_responder_se_declara_en_el_encuadre() {
        let mut a = artefacto();
        a["clarification"]["answers"] = json!([]);
        assert!(render(&a).contains("*sin respuesta del investigador*"));
    }

    #[test]
    fn la_advertencia_de_cobertura_se_imprime_con_sus_gaps() {
        let mut a = artefacto();
        a["coverage_warning"] =
            json!({"sufficient": false, "rationale": "Faltan años", "gaps": ["1967-1970"]});
        let md = render(&a);
        assert!(md.contains("> **Advertencia de cobertura.** Faltan años"));
        assert!(md.contains("> - 1967-1970"));
    }

    #[test]
    fn el_pipe_en_el_nombre_de_la_coleccion_no_rompe_la_tabla() {
        let mut a = artefacto();
        a["coverage"]["collections"][0]["name"] = json!("Volantes | Panfletos");
        assert!(render(&a).contains("| Volantes \\| Panfletos |"));
    }

    #[test]
    fn un_fragmento_recortado_avisa_que_sigue() {
        let mut a = artefacto();
        a["report"]["sections"][0]["quotes"][0]["truncated"] = json!(true);
        let md = render(&a);
        assert!(md.contains("> se resolvió la huelga general\n> […]\n"));
        assert!(!render(&artefacto()).contains("[…]"));
    }

    #[test]
    fn un_fragmento_multilinea_queda_dentro_de_la_cita() {
        let mut a = artefacto();
        a["report"]["sections"][0]["quotes"][0]["text"] = json!("primera línea\nsegunda línea");
        let md = render(&a);
        assert!(md.contains("> primera línea\n> segunda línea\n"));
    }

    #[test]
    fn sin_referencias_no_se_emite_la_seccion_de_fuentes() {
        let mut a = artefacto();
        a["report"]["sections"][0]["quotes"] = json!([]);
        a["report"]["references"] = json!([]);
        let md = render(&a);
        assert!(!md.contains("## Fuentes citadas"));
        assert!(citas_sin_referencia(&md).is_empty());
    }

    #[test]
    fn las_limitaciones_suman_los_claims_descartados_con_el_plural_correcto() {
        let mut a = artefacto();
        a["dropped_claims"] = json!([{"id": "c9", "reason": "texto vacío"}]);
        let md = render(&a);
        assert!(md.contains("- 1967 sin cobertura"));
        assert!(md.contains("1 afirmación se descartó"));
        a["dropped_claims"] = json!([{"id": "c9"}, {"id": "c10"}]);
        assert!(render(&a).contains("2 afirmaciones se descartaron"));
    }

    #[test]
    fn la_degradacion_del_pipeline_se_declara_en_las_limitaciones() {
        let mut a = artefacto();
        a["role_warnings"] = json!([
            {"role":"investigador_principal","error":"plan incompleto; se usa la pregunta como consulta","times":1},
            {"role":"asistente_archivo","error":"Salida inválida de asistente_archivo","times":3}
        ]);
        let md = render(&a);
        assert!(md.contains(
            "**Degradación del pipeline:** plan incompleto; se usa la pregunta como consulta (investigador_principal)."
        ));
        assert!(md.contains(
            "**Degradación del pipeline:** Salida inválida de asistente_archivo (asistente_archivo, 3 veces)."
        ));
    }

    #[test]
    fn sin_degradaciones_el_informe_no_habla_de_pipeline() {
        assert!(!render(&artefacto()).contains("Degradación del pipeline"));
    }

    #[test]
    fn un_corchete_en_la_respuesta_del_investigador_no_falsifica_una_cita() {
        let mut a = artefacto();
        a["clarification"]["answers"] =
            json!([{"id":"q1","text":"El tramo [3] del expediente, no el [9]."}]);
        let md = render(&a);
        assert!(md.contains("El tramo \\[3\\] del expediente"));
        assert!(
            citas_sin_referencia(&md).is_empty(),
            "el texto del investigador no puede contarse como cita"
        );
    }
}

mod common;
use entropia_agent::{
    cliente_llm::{ClienteLlm, TurnoAgente},
    estado::EstadoDb,
    investigacion::procesar,
    repositorio::RepositorioSqlite,
};
use serde_json::{json, Value};
use std::cell::Cell;
struct Model {
    calls: Cell<usize>,
    invent: bool,
    block: bool,
    /// Preguntas que devuelve la ronda de clarificación. Por debajo de cuatro
    /// el código tiene que completar con los ejes de la modalidad.
    questions: usize,
    /// El archivo cita un pasaje que no está en la fuente.
    quote_falsa: bool,
    /// El archivo no declara ningún pasaje.
    sin_pasajes: bool,
}
impl ClienteLlm for Model {
    fn modelo(&self) -> &str {
        "scripted"
    }
    fn ultimo_costo(&self) -> Option<f64> {
        Some(0.01)
    }
    fn turno_agente(&self, m: &[Value], _: &[Value]) -> Result<TurnoAgente, String> {
        self.calls.set(self.calls.get() + 1);
        let s = m[0]["content"].as_str().unwrap();
        // El verificador manda prosa («AFIRMACIÓN: … EVIDENCIA: …»), no JSON.
        let data: Value =
            serde_json::from_str(m[1]["content"].as_str().unwrap()).unwrap_or(Value::Null);
        let output = if s.contains("Rol: prospeccion.") {
            json!({"sufficient":!self.block,"rationale":"Juicio sobre el alcance documental","gaps":if self.block {vec!["1967-1970 sin cobertura identificada"]} else {vec![]}})
        } else if s.contains("{hypothesis:") {
            json!({"hypothesis":"Hubo una huelga","scope":"SOIP","closing_criteria":["Examinar los documentos recuperados"]})
        } else if s.contains("{questions:") {
            let ejes = ["Período", "Enfoque", "Fuentes", "Criterio de cierre"];
            json!({"questions": (0..self.questions).map(|i| json!({
                "id": format!("q{}", i + 1),
                "axis": ejes[i % ejes.len()],
                "text": format!("Pregunta {} al investigador", i + 1),
                "rationale": "Cambia el plan"
            })).collect::<Vec<_>>()})
        } else if s.contains("replanificá") {
            // El plan revisado se distingue del original: el test comprueba
            // que las respuestas efectivamente lo cambiaron.
            assert!(data["clarification"]["answers"].is_array());
            json!({"queries":["huelga","asamblea"],"bibliography_queries":[],"retrieval_limit":10})
        } else if s.contains("{queries:") {
            json!({"queries":["huelga"],"bibliography_queries":[],"retrieval_limit":10})
        } else if s.contains("Sos el Verificador de EntropIA.") {
            // Protocolo aislado: el verificador nunca ve la síntesis previa.
            let prosa = m[1]["content"].as_str().unwrap();
            assert!(
                !prosa.contains("Síntesis"),
                "el verificador vio la síntesis del productor"
            );
            json!({"estado":"supported","rationale":"El pasaje sostiene la afirmación","error_kind":null})
        } else if s.contains("Rol: asistente_archivo.") {
            let evidencia = data["evidence"][0]["id"].clone();
            let pasaje = if self.quote_falsa {
                json!("una frase que jamás estuvo en el documento")
            } else {
                json!("huelga general")
            };
            let quotes = if self.sin_pasajes {
                json!([])
            } else {
                json!([{"evidence_id":evidencia,"quote":pasaje}])
            };
            json!({"summary":"Síntesis","claims":[{"id":"c1","text":"Hubo una huelga","evidence_ids":[if self.invent {json!("foreign-evidence")}else{data["evidence"][0]["id"].clone()}],"quotes":quotes,"interpretative":false}]})
        } else if s.contains("Rol: asistente_bibliografia.") {
            json!({"references":[],"synthesis":"Sin consultas bibliográficas solicitadas"})
        } else if s.contains("Rol: asistente_validador.") {
            assert!(data.get("summary").is_none());
            json!({"claims":[{"id":"c1","status":"supported","rationale":"El documento afirma la huelga","evidence_ids":data["claims"][0]["evidence_ids"]}]})
        } else {
            json!({"title":"Huelga","sections":[{"title":"Hechos","text":"Hubo una huelga","claim_ids":["c1"]}]})
        };
        Ok(TurnoAgente::Texto(output.to_string()))
    }
}
fn create(db: &EstadoDb, repo: &RepositorioSqlite, m: &Model, dir: &std::path::Path) -> Value {
    procesar(db,repo,m,None,dir,json!({"op":"create","question":"¿Hubo huelga?","project":"p","collection_ids":["c-conflicto"],"max_llm_calls":30,"max_cost":1.0})).unwrap()
}
fn step(
    db: &EstadoDb,
    repo: &RepositorioSqlite,
    m: &Model,
    dir: &std::path::Path,
    id: &str,
) -> Value {
    procesar(db, repo, m, None, dir, json!({"op":"advance","job_id":id})).unwrap()
}
/// Preguntas de la ronda vigente en un snapshot.
fn round_questions(snapshot: &Value) -> Vec<Value> {
    artefacto(snapshot, "clarification_round")["questions"]
        .as_array()
        .expect("la ronda debe traer preguntas")
        .clone()
}

/// Contenido del último artefacto vigente de un tipo.
fn artefacto(snapshot: &Value, kind: &str) -> Value {
    snapshot["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .rfind(|a| a["kind"] == kind && a["obsolete"] == false)
        .unwrap_or_else(|| panic!("falta el artefacto {kind}"))["content"]
        .clone()
}

/// Emite la ronda, la responde y la cierra con la replanificación.
fn answer_round(
    db: &EstadoDb,
    repo: &RepositorioSqlite,
    m: &Model,
    dir: &std::path::Path,
    id: &str,
) -> Value {
    let asked = step(db, repo, m, dir, id);
    assert_eq!(asked["job"]["status"], "awaiting_human");
    let answers = respuestas(&round_questions(&asked));
    procesar(
        db,
        repo,
        m,
        None,
        dir,
        json!({"op":"answer","job_id":id,"answers":answers}),
    )
    .unwrap();
    step(db, repo, m, dir, id)
}

fn respuestas(questions: &[Value]) -> Vec<Value> {
    questions
        .iter()
        .map(|q| json!({"id":q["id"],"text":"1965-1966, conflicto gremial, actas de asamblea"}))
        .collect()
}

/// Corre la investigación de punta a punta respondiendo la ronda cuando frena.
fn correr(
    db: &EstadoDb,
    repo: &RepositorioSqlite,
    m: &Model,
    dir: &std::path::Path,
    id: &str,
) -> Value {
    let mut out = procesar(db, repo, m, None, dir, json!({"op":"get","job_id":id})).unwrap();
    for _ in 0..60 {
        match out["job"]["status"].as_str() {
            Some("done") => return out,
            Some("awaiting_human") => {
                let answers = respuestas(&round_questions(&out));
                out = procesar(
                    db,
                    repo,
                    m,
                    None,
                    dir,
                    json!({"op":"answer","job_id":id,"answers":answers}),
                )
                .unwrap();
            }
            _ => out = step(db, repo, m, dir, id),
        }
    }
    panic!("la investigación no cerró: {}", out["job"]["status"]);
}

fn prepare_plan(
    db: &EstadoDb,
    repo: &RepositorioSqlite,
    m: &Model,
    dir: &std::path::Path,
    id: &str,
) {
    step(db, repo, m, dir, id);
    step(db, repo, m, dir, id);
    step(db, repo, m, dir, id);
    answer_round(db, repo, m, dir, id);
}
#[test]
fn no_model_call_before_scope() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let m = Model {
        calls: Cell::new(0),
        invent: false,
        block: false,
        questions: 4,
        quote_falsa: false,
        sin_pasajes: false,
    };
    let dir = path.with_extension("artifacts");
    assert!(procesar(&db,&repo,&m,None,&dir,json!({"op":"create","question":"q","project":"p","collection_ids":["c-volantes"],"max_llm_calls":20})).is_err());
    let s = create(&db, &repo, &m, &dir);
    assert_eq!(s["job"]["status"], "running");
    assert_eq!(m.calls.get(), 0);
}
#[test]
fn la_ronda_de_preguntas_frena_el_informe_hasta_que_el_investigador_responde() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let state = path.with_extension("state.sqlite");
    let dir = path.with_extension("artifacts");
    let db = EstadoDb::abrir(state.to_str().unwrap()).unwrap();
    let m = Model {
        calls: Cell::new(0),
        invent: false,
        block: false,
        questions: 4,
        quote_falsa: false,
        sin_pasajes: false,
    };
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_string();
    // Prospección, diseño y plan corren sin intervención.
    let mut out = s;
    for _ in 0..3 {
        assert_eq!(out["job"]["status"], "running", "{out}");
        out = step(&db, &repo, &m, &dir, &id);
    }
    // La ronda frena el job: sin encuadre no se construye el informe.
    let asked = step(&db, &repo, &m, &dir, &id);
    assert_eq!(asked["job"]["status"], "awaiting_human");
    assert_eq!(asked["job"]["phase"], "clarification");
    assert!(round_questions(&asked).len() >= 4, "{asked}");
    assert!(
        procesar(
            &db,
            &repo,
            &m,
            None,
            &dir,
            json!({"op":"advance","job_id":id})
        )
        .is_err(),
        "el informe no puede arrancar con la ronda abierta"
    );
    assert!(!dir.join(&id).join("report.json").exists());
    // Con las respuestas, la investigación cierra.
    let out = correr(&db, &repo, &m, &dir, &id);
    assert_eq!(out["job"]["status"], "done");
    assert!(dir.join(&id).join("report.json").exists());
    assert!(dir.join(&id).join("report.md").exists());
}
#[test]
fn invented_evidence_is_dropped_and_recorded_but_never_becomes_claim() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("artifacts");
    let m = Model {
        calls: Cell::new(0),
        invent: true,
        block: false,
        questions: 4,
        quote_falsa: false,
        sin_pasajes: false,
    };
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap();
    prepare_plan(&db, &repo, &m, &dir, id);
    let archive = procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"advance","job_id":id}),
    )
    .unwrap();
    let artifact = archive["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == "archive_batch" && a["obsolete"] == false)
        .unwrap();
    assert!(artifact["content"]["dropped"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["reason"].as_str().unwrap().contains("no suministrada")));
    assert!(artifact["content"]["claims"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| !c["evidence_ids"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e == "foreign-evidence")));
}

#[test]
fn insufficient_prospection_is_recorded_and_the_job_continues() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let state = path.with_extension("blocked-state.sqlite");
    let db = EstadoDb::abrir(state.to_str().unwrap()).unwrap();
    let dir = path.with_extension("artifacts");
    let m = Model {
        calls: Cell::new(0),
        invent: false,
        block: true,
        questions: 4,
        quote_falsa: false,
        sin_pasajes: false,
    };
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    let judged = step(&db, &repo, &m, &dir, &id);
    assert_eq!(judged["job"]["status"], "running");
    assert_eq!(judged["job"]["llm_calls"], 1);
    let judgment = judged["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == "prospection")
        .unwrap();
    assert_eq!(judgment["content"]["sufficient"], false);
    let design = step(&db, &repo, &m, &dir, &id);
    assert_eq!(m.calls.get(), 2);
    assert!(design["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["kind"] == "design"));
}

#[test]
fn legacy_coverage_closed_job_can_continue_but_cancelled_job_cannot() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let state = path.with_extension("legacy-state.sqlite");
    let dir = path.with_extension("artifacts");
    let db = EstadoDb::abrir(state.to_str().unwrap()).unwrap();
    let m = Model {
        calls: Cell::new(0),
        invent: false,
        block: true,
        questions: 4,
        quote_falsa: false,
        sin_pasajes: false,
    };
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_string();
    step(&db, &repo, &m, &dir, &id);
    drop(db);
    {
        let conn = rusqlite::Connection::open(&state).unwrap();
        conn.execute("UPDATE jobs SET status='failed',close_reason='blocked',plan_json=json_set(plan_json,'$.step',0) WHERE id=?1",[&id]).unwrap();
        conn.execute(
            "DELETE FROM human_decisions WHERE job_id=?1 AND alcance='prospection'",
            [&id],
        )
        .unwrap();
    }
    let db = EstadoDb::abrir(state.to_str().unwrap()).unwrap();
    let resumed = procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"continue_coverage","job_id":id}),
    )
    .unwrap();
    assert_eq!(resumed["job"]["status"], "running");
    assert!(resumed["job"]["close_reason"].is_null());
    assert_eq!(m.calls.get(), 1);
    procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"cancel","job_id":id}),
    )
    .unwrap();
    assert!(procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"continue_coverage","job_id":id})
    )
    .is_err());
}

struct BoundedModel {
    base: Model,
    archive_inputs: std::cell::RefCell<Vec<String>>,
    fail_next: Cell<bool>,
    garbage_next: Cell<bool>,
}
impl ClienteLlm for BoundedModel {
    fn modelo(&self) -> &str {
        self.base.modelo()
    }
    fn ultimo_costo(&self) -> Option<f64> {
        Some(0.01)
    }
    fn turno_agente(&self, messages: &[Value], tools: &[Value]) -> Result<TurnoAgente, String> {
        let system = messages[0]["content"].as_str().unwrap();
        let text = messages[1]["content"].as_str().unwrap();
        if system.contains("Rol: asistente_archivo.") {
            assert!(
                text.len() <= 48_000,
                "archive context exceeds 48 KB: {}",
                text.len()
            );
            self.archive_inputs.borrow_mut().push(text.to_owned());
            if self.garbage_next.replace(false) {
                return Ok(TurnoAgente::Texto("NOT-JSON{{{".into()));
            }
            if self.fail_next.replace(false) {
                return Ok(TurnoAgente::Texto(
                    r#"{"summary":"Mixto","claims":[{"id":"bad","text":"Hecho","evidence_ids":["absent"],"interpretative":false},{"id":"C8","text":"No hay en el lote evidencia suficiente para las obreras","interpretative":true}]}"#.into(),
                ));
            }
            let data: Value = serde_json::from_str(text).unwrap_or(Value::Null);
            let evidencia = data["evidence"][0]["id"].clone();
            let pasaje = data["evidence"][0]["text"]
                .as_str()
                .map(|t| t.chars().take(20).collect::<String>())
                .unwrap_or_default();
            return Ok(TurnoAgente::Texto(json!({"summary":"Síntesis","claims":[{"id":"c1","text":"Hubo organización obrera","evidence_ids":[evidencia.clone()],"quotes":[{"evidence_id":evidencia,"quote":pasaje}],"interpretative":false}]}).to_string()));
        }
        if system.contains("Sos el Verificador de EntropIA.") {
            assert!(text.len() <= 52_000, "verification context is unbounded");
            return Ok(TurnoAgente::Texto(
                json!({"estado":"supported","rationale":"El pasaje sostiene la afirmación","error_kind":null})
                    .to_string(),
            ));
        }
        if system.contains("Rol: asistente_redaccion.") {
            let data: Value = serde_json::from_str(text).unwrap();
            assert!(text.len() <= 52_000, "writer context is unbounded");
            return Ok(TurnoAgente::Texto(json!({"title":"Informe","sections":[{"title":"Hechos","text":"Informe basado en los claims verificados","claim_ids":data["claims"].as_array().unwrap().iter().map(|c|c["id"].clone()).collect::<Vec<_>>()}]}).to_string()));
        }
        self.base.turno_agente(messages, tools)
    }
}

#[test]
fn large_archive_checkpoints_invalid_claims_instead_of_blocking_the_batch() {
    let path = common::crear_corpus_sintetico();
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        let text = "huelga y organización obrera. ".repeat(12000);
        conn.execute(
            "UPDATE rag_chunks SET text_content=?1 WHERE item_id='item-1'",
            [&text],
        )
        .unwrap();
        conn.execute("UPDATE rag_chunks_fts SET text_content=?1 WHERE chunk_id IN (SELECT id FROM rag_chunks WHERE item_id='item-1')",[&text]).unwrap();
    }
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let state = path.with_extension("batch-state.sqlite");
    let dir = path.with_extension("artifacts");
    let db = EstadoDb::abrir(state.to_str().unwrap()).unwrap();
    let base = Model {
        calls: Cell::new(0),
        invent: false,
        block: false,
        questions: 4,
        quote_falsa: false,
        sin_pasajes: false,
    };
    let s = create(&db, &repo, &base, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    prepare_plan(&db, &repo, &base, &dir, &id);
    let model = BoundedModel {
        base,
        archive_inputs: Default::default(),
        fail_next: Cell::new(false),
        garbage_next: Cell::new(false),
    };
    let first = procesar(
        &db,
        &repo,
        &model,
        None,
        &dir,
        json!({"op":"advance","job_id":id}),
    )
    .unwrap();
    assert!(first["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["kind"] == "archive_batch"));
    assert_eq!(first["job"]["phase"], "execution");
    drop(db);
    let db = EstadoDb::abrir(state.to_str().unwrap()).unwrap();
    model.fail_next.set(true);
    let mixed = procesar(
        &db,
        &repo,
        &model,
        None,
        &dir,
        json!({"op":"advance","job_id":id}),
    )
    .unwrap();
    let batch = mixed["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .rfind(|a| a["kind"] == "archive_batch" && a["obsolete"] == false)
        .unwrap();
    assert!(batch["content"]["dropped"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["reason"].as_str().unwrap().contains("absent")));
    assert!(batch["content"]["limitations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l["text"].as_str().unwrap().contains("obreras")));
    assert!(!mixed["job"]["status"].as_str().unwrap().contains("paused"));
    assert!(mixed["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["kind"] == "output_asistente_archivo"
            && a["content"]["raw"]
                .as_str()
                .is_some_and(|s| s.contains("absent"))));
    assert_ne!(
        model.archive_inputs.borrow()[0],
        model.archive_inputs.borrow()[1]
    );
    procesar(
        &db,
        &repo,
        &model,
        None,
        &dir,
        json!({"op":"pause","job_id":id}),
    )
    .unwrap();
    assert!(procesar(
        &db,
        &repo,
        &model,
        None,
        &dir,
        json!({"op":"update_budget","job_id":id,"max_llm_calls":1,"max_cost":1})
    )
    .is_err());
    let adjusted = procesar(
        &db,
        &repo,
        &model,
        None,
        &dir,
        json!({"op":"update_budget","job_id":id,"max_llm_calls":60,"max_cost":2}),
    )
    .unwrap();
    assert_eq!(adjusted["job"]["status"], "paused");
    assert_eq!(adjusted["job"]["max_llm_calls"], 60);
    assert_eq!(adjusted["job"]["max_cost"], 2.0);
    assert!(adjusted["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "budget_updated"));
    procesar(
        &db,
        &repo,
        &model,
        None,
        &dir,
        json!({"op":"resume","job_id":id}),
    )
    .unwrap();
    let mut result = procesar(
        &db,
        &repo,
        &model,
        None,
        &dir,
        json!({"op":"advance","job_id":id}),
    )
    .unwrap();
    assert_ne!(
        model.archive_inputs.borrow()[0],
        model.archive_inputs.borrow()[1]
    );
    for _ in 0..100 {
        if result["job"]["status"] == "done" {
            break;
        }
        if result["job"]["status"] == "awaiting_human" {
            let gate = result["gates"]
                .as_array()
                .unwrap()
                .iter()
                .find(|g| g["status"] == "pending")
                .unwrap();
            procesar(
                &db,
                &repo,
                &model,
                None,
                &dir,
                json!({"op":"decision","job_id":id,"gate_id":gate["id"],"approve":true}),
            )
            .unwrap();
        }
        result = procesar(
            &db,
            &repo,
            &model,
            None,
            &dir,
            json!({"op":"advance","job_id":id}),
        )
        .unwrap();
    }
    assert_eq!(result["job"]["status"], "done");
    assert!(dir.join(id).join("report.json").exists());
}

#[test]
fn garbage_archive_json_is_recorded_and_does_not_pause() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let state = path.with_extension("garbage-state.sqlite");
    let dir = path.with_extension("garbage-artifacts");
    let db = EstadoDb::abrir(state.to_str().unwrap()).unwrap();
    let base = Model {
        calls: Cell::new(0),
        invent: false,
        block: false,
        questions: 4,
        quote_falsa: false,
        sin_pasajes: false,
    };
    let s = create(&db, &repo, &base, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    prepare_plan(&db, &repo, &base, &dir, &id);
    let model = BoundedModel {
        base,
        archive_inputs: Default::default(),
        fail_next: Cell::new(false),
        garbage_next: Cell::new(true),
    };
    let out = procesar(
        &db,
        &repo,
        &model,
        None,
        &dir,
        json!({"op":"advance","job_id":id}),
    )
    .unwrap();
    assert!(!out["job"]["status"].as_str().unwrap().contains("paused"));
    assert!(out["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "role_warning"));
    let batch = out["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .rfind(|a| a["kind"] == "archive_batch" && a["obsolete"] == false)
        .unwrap();
    assert!(batch["content"]["limitations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l["text"].as_str().unwrap().contains("no parseable")));
}

#[test]
fn una_ronda_corta_se_completa_con_los_ejes_de_la_modalidad() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("cortas-artifacts");
    let m = Model {
        calls: Cell::new(0),
        invent: false,
        block: false,
        questions: 1,
        quote_falsa: false,
        sin_pasajes: false,
    };
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    let asked = step(&db, &repo, &m, &dir, &id);
    let questions = round_questions(&asked);
    assert_eq!(questions.len(), 4, "el piso de cuatro preguntas es firme");
    let ids: std::collections::HashSet<&str> =
        questions.iter().filter_map(|q| q["id"].as_str()).collect();
    assert_eq!(ids.len(), 4, "los identificadores no pueden repetirse");
    assert!(asked["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "role_warning"
            && e["payload"]["error"]
                .as_str()
                .unwrap()
                .contains("menos de 4 preguntas")));
}

#[test]
fn las_respuestas_del_investigador_regeneran_el_plan() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("replan-artifacts");
    let m = Model {
        calls: Cell::new(0),
        invent: false,
        block: false,
        questions: 4,
        quote_falsa: false,
        sin_pasajes: false,
    };
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    let planned = step(&db, &repo, &m, &dir, &id);
    assert_eq!(artefacto(&planned, "plan")["queries"], json!(["huelga"]));
    let closed = answer_round(&db, &repo, &m, &dir, &id);
    // El plan revisado convive con el original: se versiona, no se pisa.
    assert_eq!(
        artefacto(&closed, "plan")["queries"],
        json!(["huelga", "asamblea"])
    );
    let versiones = closed["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["kind"] == "plan")
        .count();
    assert_eq!(versiones, 2);
    // El encuadre queda registrado con pregunta y respuesta.
    let ronda = artefacto(&closed, "clarification");
    assert_eq!(ronda["answers"].as_array().unwrap().len(), 4);
    assert_eq!(closed["job"]["phase"], "execution");
}

#[test]
fn answer_rechaza_preguntas_ajenas_rondas_repetidas_y_encuadres_vacios() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("answer-artifacts");
    let m = Model {
        calls: Cell::new(0),
        invent: false,
        block: false,
        questions: 4,
        quote_falsa: false,
        sin_pasajes: false,
    };
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    // Antes de la ronda no hay nada que responder.
    assert!(procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"answer","job_id":id,"answers":[{"id":"q1","text":"x"}]})
    )
    .is_err());
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    let asked = step(&db, &repo, &m, &dir, &id);
    let questions = round_questions(&asked);
    // Una pregunta que no es de esta ronda.
    assert!(procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"answer","job_id":id,"answers":[{"id":"inventada","text":"x"}]})
    )
    .is_err());
    // Dos respuestas para la misma pregunta.
    assert!(procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"answer","job_id":id,"answers":[{"id":"q1","text":"a"},{"id":"q1","text":"b"}]})
    )
    .is_err());
    // Una ronda entera en blanco no es un encuadre.
    let vacias: Vec<Value> = questions
        .iter()
        .map(|q| json!({"id":q["id"],"text":"   "}))
        .collect();
    assert!(procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"answer","job_id":id,"answers":vacias})
    )
    .is_err());
    assert!(procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"answer","job_id":id,"answers":[]})
    )
    .is_err());
    // Una pregunta suelta sin responder sí se acepta y queda declarada.
    let parciales: Vec<Value> = questions
        .iter()
        .enumerate()
        .map(|(i, q)| json!({"id":q["id"],"text":if i == 0 {"1965-1966"} else {""}}))
        .collect();
    let respondida = procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"answer","job_id":id,"answers":parciales}),
    )
    .unwrap();
    assert_eq!(respondida["job"]["status"], "running");
    // La misma ronda no se responde dos veces.
    assert!(procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"answer","job_id":id,"answers":respuestas(&questions)})
    )
    .is_err());
}

#[test]
fn el_informe_reproduce_los_fragmentos_literales_y_cierra_con_fuentes_citadas() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("citas-artifacts");
    let m = Model {
        calls: Cell::new(0),
        invent: false,
        block: false,
        questions: 4,
        quote_falsa: false,
        sin_pasajes: false,
    };
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    let out = correr(&db, &repo, &m, &dir, &id);
    let evidencia = artefacto(&out, "archive")["evidence"].clone();
    let informe = artefacto(&out, "report");
    let secciones = informe["report"]["sections"].as_array().unwrap();
    let referencias = informe["report"]["references"].as_array().unwrap();
    assert!(!referencias.is_empty(), "el informe no citó ninguna fuente");

    let mut usados = std::collections::BTreeSet::new();
    for seccion in secciones {
        for cita in seccion["quotes"].as_array().unwrap() {
            usados.insert(cita["n"].as_i64().unwrap());
            // El fragmento es literal: sale del artefacto, no del modelo.
            let fuente = evidencia
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["id"] == cita["evidence_id"])
                .expect("la cita referencia evidencia del archivo");
            assert!(fuente["text"]
                .as_str()
                .unwrap()
                .contains(cita["text"].as_str().unwrap()));
            assert_eq!(cita["collection"], "Conflicto SOIP 1965-66");
            assert!(!cita["title"].as_str().unwrap().is_empty());
        }
    }
    let declarados: std::collections::BTreeSet<i64> = referencias
        .iter()
        .map(|r| r["n"].as_i64().unwrap())
        .collect();
    assert_eq!(usados, declarados, "hay números citados sin referencia");
    assert_eq!(
        declarados.iter().copied().collect::<Vec<_>>(),
        (1..=declarados.len() as i64).collect::<Vec<_>>(),
        "la numeración tiene que ser correlativa desde 1"
    );

    let md = std::fs::read_to_string(dir.join(&id).join("report.md")).unwrap();
    assert!(md.contains("## Cobertura del recorte consultado"));
    assert!(md.contains("## Fuentes citadas"));
    // El pasaje verificado, no el fragmento entero.
    assert!(md.contains("> huelga general"));
    assert!(md.contains("## Encuadre acordado con el investigador"));
    assert!(
        entropia_agent::informe_render::citas_sin_referencia(&md).is_empty(),
        "quedó un [n] sin su línea de referencia:\n{md}"
    );
}

#[test]
fn sin_claims_verificados_el_informe_no_inventa_fuentes_citadas() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("sin-citas-artifacts");
    // `invent` hace que el archivo referencie evidencia no suministrada: todos
    // los claims se descartan y no queda nada verificado que citar.
    let m = Model {
        calls: Cell::new(0),
        invent: true,
        block: false,
        questions: 4,
        quote_falsa: false,
        sin_pasajes: false,
    };
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    let out = correr(&db, &repo, &m, &dir, &id);
    let informe = artefacto(&out, "report");
    assert!(informe["report"]["references"]
        .as_array()
        .unwrap()
        .is_empty());
    let md = std::fs::read_to_string(dir.join(&id).join("report.md")).unwrap();
    assert!(!md.contains("## Fuentes citadas"));
    assert!(entropia_agent::informe_render::citas_sin_referencia(&md).is_empty());
}

#[test]
fn la_modalidad_perfila_la_ronda_y_queda_declarada_en_el_informe() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("perfil-artifacts");
    let m = Model {
        calls: Cell::new(0),
        invent: false,
        block: false,
        questions: 0,
        quote_falsa: false,
        sin_pasajes: false,
    };
    // Una modalidad que no está en la tabla no crea el job.
    assert!(procesar(&db,&repo,&m,None,&dir,json!({"op":"create","question":"¿Hubo huelga?","project":"p","collection_ids":["c-conflicto"],"max_llm_calls":30,"max_cost":1.0,"modalidad":"biografia-inventada"})).is_err());
    let s = procesar(&db,&repo,&m,None,&dir,json!({"op":"create","question":"¿Hubo huelga?","project":"p","collection_ids":["c-conflicto"],"max_llm_calls":30,"max_cost":1.0,"modalidad":"cronologia"})).unwrap();
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    // El modelo no aportó ninguna pregunta: las cuatro salen de la modalidad.
    let asked = step(&db, &repo, &m, &dir, &id);
    let questions = round_questions(&asked);
    assert_eq!(questions.len(), 4);
    let ejes: Vec<&str> = questions
        .iter()
        .filter_map(|q| q["axis"].as_str())
        .collect();
    assert!(ejes.contains(&"Terminus a quo y ad quem"), "{ejes:?}");
    assert!(ejes.contains(&"Granularidad"), "{ejes:?}");
    let out = correr(&db, &repo, &m, &dir, &id);
    let informe = artefacto(&out, "report");
    assert_eq!(informe["profile"]["id"], "cronologia");
    let md = std::fs::read_to_string(dir.join(&id).join("report.md")).unwrap();
    assert!(md.contains("**Perfil de informe:** Cronología y crónica de eventos y procesos"));
    assert!(md.contains("**Sesgo declarado del perfil:**"));
}

#[test]
fn revisar_el_plan_reabre_la_ronda_de_preguntas() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("revise-artifacts");
    let m = Model {
        calls: Cell::new(0),
        invent: false,
        block: false,
        questions: 4,
        quote_falsa: false,
        sin_pasajes: false,
    };
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    let cerrada = answer_round(&db, &repo, &m, &dir, &id);
    let plan = cerrada["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .rfind(|a| a["kind"] == "plan" && a["obsolete"] == false)
        .unwrap()["id"]
        .clone();
    procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"pause","job_id":id}),
    )
    .unwrap();
    let revisado = procesar(&db,&repo,&m,None,&dir,json!({"op":"revise","job_id":id,"artifact_id":plan,"content":{"queries":["paro"],"bibliography_queries":[],"retrieval_limit":5}})).unwrap();
    assert_eq!(revisado["job"]["phase"], "clarification");
    // La ronda anterior quedó obsoleta: el encuadre se vuelve a preguntar.
    assert!(revisado["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["kind"] == "clarification_round")
        .all(|a| a["obsolete"] == true));
}

#[test]
fn la_degradacion_de_un_rol_llega_al_informe_en_vez_de_quedar_solo_en_los_eventos() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("degradacion-artifacts");
    // El modelo no aporta preguntas: la ronda se completa con los ejes y eso
    // queda registrado como degradación.
    let m = Model {
        calls: Cell::new(0),
        invent: false,
        block: false,
        questions: 0,
        quote_falsa: false,
        sin_pasajes: false,
    };
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    let out = correr(&db, &repo, &m, &dir, &id);
    let informe = artefacto(&out, "report");
    let degradaciones = informe["role_warnings"].as_array().unwrap();
    assert!(
        degradaciones
            .iter()
            .any(|w| w["error"].as_str().unwrap().contains("preguntas usables")),
        "{degradaciones:?}"
    );
    assert!(degradaciones
        .iter()
        .all(|w| w["times"].as_i64().unwrap() >= 1));
    let md = std::fs::read_to_string(dir.join(&id).join("report.md")).unwrap();
    assert!(
        md.contains("**Degradación del pipeline:**"),
        "el informe tiene que declarar que el encuadre lo completó el código:\n{md}"
    );
}

/// Embedder de prueba: vector fijo, alineado con los embeddings del corpus
/// sintético, para ejercitar la pierna semántica sin salir a la red.
struct EmbedFijo;
impl entropia_agent::recuperacion::Embedder for EmbedFijo {
    fn embed(&self, _: &str) -> Result<Vec<f32>, String> {
        Ok(vec![1.0, 0.0])
    }
}

/// Reranker de prueba: conserva el orden de la fusión.
struct RerankIdentidad;
impl entropia_agent::recuperacion::Reranker for RerankIdentidad {
    fn rerank(&self, _: &str, docs: &[String], limite: usize) -> Result<Vec<(usize, f64)>, String> {
        Ok((0..docs.len().min(limite)).map(|i| (i, 1.0)).collect())
    }
}

/// Corre la investigación entera con el recuperador dado, respondiendo la
/// ronda cuando frena.
fn correr_con(
    db: &EstadoDb,
    repo: &RepositorioSqlite,
    m: &Model,
    rec: Option<&entropia_agent::recuperacion::Recuperador>,
    dir: &std::path::Path,
    id: &str,
) -> Value {
    let mut out = procesar(db, repo, m, rec, dir, json!({"op":"get","job_id":id})).unwrap();
    for _ in 0..60 {
        match out["job"]["status"].as_str() {
            Some("done") => return out,
            Some("awaiting_human") => {
                let answers = respuestas(&round_questions(&out));
                out = procesar(
                    db,
                    repo,
                    m,
                    rec,
                    dir,
                    json!({"op":"answer","job_id":id,"answers":answers}),
                )
                .unwrap();
            }
            _ => {
                out = procesar(db, repo, m, rec, dir, json!({"op":"advance","job_id":id})).unwrap()
            }
        }
    }
    panic!("la investigación no cerró: {}", out["job"]["status"]);
}

#[test]
fn con_recuperador_la_evidencia_sale_del_pipeline_hibrido_y_del_recorte() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("hibrida-artifacts");
    let m = Model {
        calls: Cell::new(0),
        invent: false,
        block: false,
        questions: 4,
        quote_falsa: false,
        sin_pasajes: false,
    };
    let rec = entropia_agent::recuperacion::Recuperador::con_clientes(
        Box::new(EmbedFijo),
        Box::new(RerankIdentidad),
    );
    let s = procesar(&db,&repo,&m,Some(&rec),&dir,json!({"op":"create","question":"¿Hubo huelga?","project":"p","collection_ids":["c-conflicto"],"max_llm_calls":30,"max_cost":1.0})).unwrap();
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    let out = correr_con(&db, &repo, &m, Some(&rec), &dir, &id);

    // El evento de consulta declara con qué pipeline se buscó.
    let consultas: Vec<&Value> = out["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "query")
        .collect();
    assert!(!consultas.is_empty());
    assert!(consultas
        .iter()
        .all(|e| e["payload"]["pipeline"] == "hibrida"));

    // Con las dos piernas y el rerank no hay nada que declarar.
    assert!(!out["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "retrieval_degraded"));

    // Toda la evidencia pertenece al recorte que el investigador eligió.
    let evidencia = artefacto(&out, "archive")["evidence"].clone();
    let filas = evidencia.as_array().unwrap();
    assert!(!filas.is_empty(), "la recuperación híbrida no trajo nada");
    for f in filas {
        assert_eq!(f["collection_id"], "c-conflicto", "{f}");
    }

    let md = std::fs::read_to_string(dir.join(&id).join("report.md")).unwrap();
    assert!(
        !md.contains("solo léxica"),
        "no hubo degradación que declarar"
    );
}

#[test]
fn sin_recuperador_el_informe_declara_que_busco_solo_por_lexico() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("lexica-artifacts");
    let m = Model {
        calls: Cell::new(0),
        invent: false,
        block: false,
        questions: 4,
        quote_falsa: false,
        sin_pasajes: false,
    };
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    let out = correr(&db, &repo, &m, &dir, &id);

    assert!(out["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "retrieval_degraded"));
    let informe = artefacto(&out, "report");
    assert!(
        informe["role_warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["role"] == "recuperacion"),
        "{}",
        informe["role_warnings"]
    );
    // Y el investigador lo lee en el documento, no en un log.
    let md = std::fs::read_to_string(dir.join(&id).join("report.md")).unwrap();
    assert!(
        md.contains("**Degradación del pipeline:** recuperación solo léxica"),
        "{md}"
    );
}

/// Juicio del claim `c1` en el artefacto de verificación.
fn juicio(out: &Value) -> Value {
    artefacto(out, "verification")["claims"]
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["id"] == "c1")
        .expect("c1 tiene que tener juicio")
        .clone()
}

fn modelo(quote_falsa: bool, sin_pasajes: bool) -> Model {
    Model {
        calls: Cell::new(0),
        invent: false,
        block: false,
        questions: 4,
        quote_falsa,
        sin_pasajes,
    }
}

#[test]
fn una_cita_que_no_esta_en_la_fuente_no_pasa_el_span_check() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("span-artifacts");
    let m = modelo(true, false);
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    let out = correr(&db, &repo, &m, &dir, &id);

    // El archivo guarda el pasaje tal como lo dijo el modelo, pero sin
    // posición: no está en la fuente.
    let claim = artefacto(&out, "archive")["claims"][0].clone();
    assert!(claim["quotes"][0]["span_start"].is_null(), "{claim}");

    // Y la verificación lo declara, en vez de darlo por bueno.
    let j = juicio(&out);
    assert_eq!(j["status"], "unverifiable", "{j}");
    assert_eq!(j["error_kind"], "ref_conflict", "{j}");

    // Sin claim sostenido no hay nada que citar: el informe no inventa fuentes.
    let informe = artefacto(&out, "report");
    assert!(informe["report"]["references"]
        .as_array()
        .unwrap()
        .is_empty());
    let md = std::fs::read_to_string(dir.join(&id).join("report.md")).unwrap();
    assert!(!md.contains("## Fuentes citadas"));
}

#[test]
fn un_claim_sin_pasaje_declarado_no_se_da_por_verificado() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("sin-pasaje-artifacts");
    let m = modelo(false, true);
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    let out = correr(&db, &repo, &m, &dir, &id);
    let j = juicio(&out);
    // Verificar una cita vacía contra su fuente pasaría siempre: el guardrail
    // tiene que rechazar antes de llegar al span check.
    assert_eq!(j["status"], "unverifiable", "{j}");
    assert_eq!(j["error_kind"], "knowledge_lack", "{j}");
    assert!(j["rationale"].as_str().unwrap().contains("ningún pasaje"));
}

#[test]
fn con_pasaje_literal_el_claim_queda_sostenido_y_se_cita() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("span-ok-artifacts");
    let m = modelo(false, false);
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    let out = correr(&db, &repo, &m, &dir, &id);

    let claim = artefacto(&out, "archive")["claims"][0].clone();
    assert_eq!(claim["quotes"][0]["quote"], "huelga general");
    assert_eq!(claim["quotes"][0]["span_start"], 0);
    assert_eq!(claim["quotes"][0]["span_end"], 14);

    let j = juicio(&out);
    assert_eq!(j["status"], "supported", "{j}");
    assert!(j["error_kind"].is_null());

    // El informe reproduce el pasaje verificado, no una ventana ciega del
    // principio del fragmento.
    let informe = artefacto(&out, "report");
    assert!(!informe["report"]["references"]
        .as_array()
        .unwrap()
        .is_empty());
    let cita = informe["report"]["sections"][0]["quotes"][0].clone();
    assert_eq!(cita["text"], "huelga general", "{cita}");
    assert_eq!(cita["start"], 0);
    assert_eq!(cita["end"], 14);
    let md = std::fs::read_to_string(dir.join(&id).join("report.md")).unwrap();
    assert!(md.contains("> huelga general\n"), "{md}");
    assert!(
        !md.contains("huelga general de la pesca en marzo"),
        "se imprimió el fragmento entero en vez del pasaje citado:\n{md}"
    );
}

#[test]
fn el_presupuesto_agotado_degrada_la_verificacion_en_vez_de_saltearla() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("presupuesto-artifacts");
    let m = modelo(false, false);
    // Alcanza para llegar a la verificación y no para pagarla.
    let s = procesar(&db,&repo,&m,None,&dir,json!({"op":"create","question":"¿Hubo huelga?","project":"p","collection_ids":["c-conflicto"],"max_llm_calls":7,"max_cost":1.0})).unwrap();
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    let mut out = s;
    for _ in 0..12 {
        if out["job"]["status"] == "awaiting_human" {
            let answers = respuestas(&round_questions(&out));
            out = procesar(
                &db,
                &repo,
                &m,
                None,
                &dir,
                json!({"op":"answer","job_id":id,"answers":answers}),
            )
            .unwrap();
            continue;
        }
        match procesar(
            &db,
            &repo,
            &m,
            None,
            &dir,
            json!({"op":"advance","job_id":id}),
        ) {
            Ok(v) => out = v,
            Err(_) => break,
        }
    }
    let out = procesar(&db, &repo, &m, None, &dir, json!({"op":"get","job_id":id})).unwrap();
    // La verificación corrió igual, con span check y entailment determinista.
    let j = juicio(&out);
    assert!(j["status"].is_string(), "{j}");
    assert!(out["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "role_warning"
            && e["payload"]["error"]
                .as_str()
                .unwrap()
                .contains("presupuesto agotado")));
}

#[test]
fn la_investigacion_queda_consultable_en_el_ledger_relacional() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("ledger-artifacts");
    let m = modelo(false, false);
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    let out = correr(&db, &repo, &m, &dir, &id);

    let claim = artefacto(&out, "archive")["claims"][0].clone();
    let ledger_id = claim["ledger_id"]
        .as_str()
        .expect("el claim tiene que quedar asentado en el ledger");
    let ledger = entropia_agent::dominio::Ledger::nuevo(&db);

    // El claim existe como fila, no solo adentro de un JSON.
    let asentado = ledger
        .claim(ledger_id)
        .expect("el claim existe en el ledger");
    assert_eq!(asentado.texto, "Hubo una huelga");
    assert_eq!(asentado.job_id, id);
    assert_eq!(asentado.tipo, "factual");

    // Y su estado epistémico se proyecta desde el run aceptado.
    assert_eq!(
        ledger.estado_epistemico(ledger_id),
        Some(entropia_agent::dominio::EstadoEpistemico::Supported)
    );
    let run = ledger
        .verificacion_vigente(ledger_id)
        .expect("hay una verificación vigente");
    assert!(!run.obsoleto);
    assert!(run.prompt_hash.is_some());

    // La evidencia quedó ligada con su pasaje y su span verificado.
    let evidencias = ledger.evidencias_del_claim(ledger_id);
    assert_eq!(evidencias.len(), 1, "{evidencias:?}");
    let (evidencia, relacion, _) = &evidencias[0];
    assert_eq!(evidencia.quote, "huelga general");
    assert_eq!((evidencia.span_start, evidencia.span_end), (0, 14));
    assert_eq!(relacion, "supports");

    // Y la fuente conserva su procedencia en el corpus.
    let fuente = ledger
        .fuente(&evidencia.source_id)
        .expect("la evidencia apunta a una fuente registrada");
    assert_eq!(fuente.kind, "entropia_chunk");
    assert_eq!(fuente.item_id.as_deref(), Some("item-1"));
    assert_eq!(fuente.chunk_id.as_deref(), Some("chunk-1"));
}

#[test]
fn una_cita_sin_span_no_se_asienta_como_evidencia() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("ledger-falsa-artifacts");
    let m = modelo(true, false);
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    let out = correr(&db, &repo, &m, &dir, &id);

    let ledger_id = artefacto(&out, "archive")["claims"][0]["ledger_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ledger = entropia_agent::dominio::Ledger::nuevo(&db);
    // El claim se asienta —existió— pero sin evidencia: el ledger solo guarda
    // citas con span verificable.
    assert!(ledger.claim(&ledger_id).is_some());
    assert!(ledger.evidencias_del_claim(&ledger_id).is_empty());
    assert_eq!(
        ledger.estado_epistemico(&ledger_id),
        Some(entropia_agent::dominio::EstadoEpistemico::Unverifiable)
    );
}

#[test]
fn revisar_el_plan_invalida_las_verificaciones_ya_asentadas() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("invalidacion-artifacts");
    let m = modelo(false, false);
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();

    // Avanzar hasta tener verificación, sin llegar al informe.
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    answer_round(&db, &repo, &m, &dir, &id);
    let mut out = json!(null);
    for _ in 0..6 {
        out = step(&db, &repo, &m, &dir, &id);
        if out["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["kind"] == "verification")
        {
            break;
        }
    }
    let ledger_id = artefacto(&out, "archive")["claims"][0]["ledger_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ledger = entropia_agent::dominio::Ledger::nuevo(&db);
    assert!(
        ledger.verificacion_vigente(&ledger_id).is_some(),
        "la verificación tiene que estar vigente antes de revisar"
    );

    // Revisar el plan invalida todo lo derivado, incluidas las verificaciones.
    procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"pause","job_id":id}),
    )
    .unwrap();
    let plan = out["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .rfind(|a| a["kind"] == "plan" && a["obsolete"] == false)
        .unwrap()["id"]
        .clone();
    procesar(&db,&repo,&m,None,&dir,json!({"op":"revise","job_id":id,"artifact_id":plan,"content":{"queries":["paro"],"bibliography_queries":[],"retrieval_limit":5}})).unwrap();

    // El run sigue existiendo (append-only) pero ya no proyecta.
    assert!(
        ledger.verificacion_vigente(&ledger_id).is_none(),
        "revisar el plan tiene que invalidar la verificación derivada"
    );
    assert!(
        !ledger.runs_del_claim(&ledger_id).is_empty(),
        "el run no se borra"
    );
    assert!(ledger.runs_del_claim(&ledger_id).iter().all(|r| r.obsoleto));
}

/// Contenidos de todos los artefactos de entrada de un rol.
fn entradas(out: &Value, rol: &str) -> Vec<Value> {
    out["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["kind"] == format!("input_{rol}"))
        .map(|a| a["content"].clone())
        .collect()
}

#[test]
fn la_memoria_longitudinal_alimenta_la_investigacion_siguiente() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("memoria-artifacts");
    let m = modelo(false, false);

    // Primera investigación: deja sus hallazgos sostenidos.
    let uno = create(&db, &repo, &m, &dir);
    let id1 = uno["job"]["id"].as_str().unwrap().to_owned();
    let cerrada = correr(&db, &repo, &m, &dir, &id1);
    assert!(cerrada["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "memory_saved"));

    let memoria = entropia_agent::memoria::MemoriaDb::nuevo(&db);
    let recordado = memoria.buscar("p", "huelga", 5);
    assert!(
        recordado.iter().any(|r| r.content == "Hubo una huelga"),
        "{recordado:?}"
    );

    // Segunda investigación en el mismo proyecto: el diseño ve lo anterior.
    let dos = create(&db, &repo, &m, &dir);
    let id2 = dos["job"]["id"].as_str().unwrap().to_owned();
    step(&db, &repo, &m, &dir, &id2);
    let disenada = step(&db, &repo, &m, &dir, &id2);
    let vio_memoria = entradas(&disenada, "investigador_principal")
        .iter()
        .any(|e| {
            e["data"]["memory"]
                .as_array()
                .is_some_and(|m| !m.is_empty())
        });
    assert!(
        vio_memoria,
        "el diseño tiene que recibir los hallazgos previos"
    );
}

#[test]
fn los_hallazgos_previos_nunca_llegan_al_archivo_ni_a_la_verificacion() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("frontera-artifacts");
    let m = modelo(false, false);

    let uno = create(&db, &repo, &m, &dir);
    let id1 = uno["job"]["id"].as_str().unwrap().to_owned();
    correr(&db, &repo, &m, &dir, &id1);

    let dos = create(&db, &repo, &m, &dir);
    let id2 = dos["job"]["id"].as_str().unwrap().to_owned();
    let out = correr(&db, &repo, &m, &dir, &id2);

    // Provenance clase 2: un informe previo del agente es contexto de trabajo.
    // Si llegara al archivo o al verificador, una afirmación podría sostenerse
    // en el informe anterior del propio agente en vez de en el corpus.
    for rol in ["asistente_archivo", "asistente_validador"] {
        let recibidas = entradas(&out, rol);
        assert!(
            !recibidas.is_empty(),
            "{rol} no registró ninguna entrada: el test pasaría en vacío"
        );
        for entrada in recibidas {
            assert!(
                !entrada.to_string().contains("memory"),
                "{rol} recibió memoria longitudinal: {entrada}"
            );
        }
    }
}

#[test]
fn solo_se_recuerda_lo_que_quedo_sostenido() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("memoria-falsa-artifacts");
    // Cita fabricada: el claim queda unverifiable y no sostiene nada.
    let m = modelo(true, false);
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    correr(&db, &repo, &m, &dir, &id);

    let memoria = entropia_agent::memoria::MemoriaDb::nuevo(&db);
    let recordado = memoria.buscar("p", "huelga", 5);
    assert!(
        !recordado.iter().any(|r| r.content == "Hubo una huelga"),
        "una conjetura no verificada no puede volver como hallazgo: {recordado:?}"
    );
}

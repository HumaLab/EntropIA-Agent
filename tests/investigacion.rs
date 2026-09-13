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
    /// La replanificación devuelve el mismo plan que ya había. Es el caso que
    /// el resto de la suíte no puede producir: su modelo falso siempre cambia
    /// el plan, así que la rama en que el encuadre no surte efecto nunca se
    /// ejercitó.
    replan_identico: bool,
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
        } else if s.contains("replanificá") && self.replan_identico {
            // Devuelve exactamente el plan de `{queries:`: el encuadre no
            // dejó huella.
            json!({"queries":["huelga"],"bibliography_queries":[],"retrieval_limit":10})
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
/// Modelo que propone sus propios planes y delega el resto de los roles en
/// `Model`. Sirve para ejercitar lo que el código hace con un plan que el
/// modelo propone fuera del contrato.
struct PlanPropio {
    base: Model,
    /// Lo que devuelve el paso de plan.
    plan: Value,
    /// Lo que devuelve la replanificación tras la ronda de clarificación.
    replan: Value,
}
impl ClienteLlm for PlanPropio {
    fn modelo(&self) -> &str {
        self.base.modelo()
    }
    fn ultimo_costo(&self) -> Option<f64> {
        self.base.ultimo_costo()
    }
    fn turno_agente(&self, m: &[Value], t: &[Value]) -> Result<TurnoAgente, String> {
        let s = m[0]["content"].as_str().unwrap();
        if s.contains("replanificá") {
            Ok(TurnoAgente::Texto(self.replan.to_string()))
        } else if s.contains("{queries:") {
            Ok(TurnoAgente::Texto(self.plan.to_string()))
        } else {
            self.base.turno_agente(m, t)
        }
    }
}
fn create(
    db: &EstadoDb,
    repo: &RepositorioSqlite,
    m: &dyn ClienteLlm,
    dir: &std::path::Path,
) -> Value {
    procesar(db,repo,m,None,dir,json!({"op":"create","question":"¿Hubo huelga?","project":"p","collection_ids":["c-conflicto"],"max_llm_calls":30,"max_cost":1.0})).unwrap()
}
fn step(
    db: &EstadoDb,
    repo: &RepositorioSqlite,
    m: &dyn ClienteLlm,
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
    m: &dyn ClienteLlm,
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

/// Destraba un job en `awaiting_human`: aprueba el gate del plan si es lo
/// que espera, o responde la ronda de preguntas.
fn destrabar(
    db: &EstadoDb,
    repo: &RepositorioSqlite,
    m: &dyn ClienteLlm,
    rec: Option<&entropia_agent::recuperacion::Recuperador>,
    dir: &std::path::Path,
    id: &str,
    out: &Value,
) -> Value {
    let pedido = match gate_pendiente(out) {
        Some(g) if g["kind"] == "plan" => {
            json!({"op":"decision","job_id":id,"gate_id":g["id"],"approve":true})
        }
        _ => json!({"op":"answer","job_id":id,"answers":respuestas(&round_questions(out))}),
    };
    procesar(db, repo, m, rec, dir, pedido).unwrap()
}

/// Corre la investigación de punta a punta respondiendo la ronda y aprobando
/// el gate del plan cuando frena.
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
            Some("awaiting_human") => out = destrabar(db, repo, m, None, dir, id, &out),
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
    let cerrada = answer_round(db, repo, m, dir, id);
    destrabar(db, repo, m, None, dir, id, &cerrada);
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
        replan_identico: false,
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
        replan_identico: false,
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
        replan_identico: false,
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
        replan_identico: false,
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
        replan_identico: false,
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
        replan_identico: false,
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
        replan_identico: false,
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
        replan_identico: false,
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
        replan_identico: false,
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
        replan_identico: false,
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
        replan_identico: false,
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
        replan_identico: false,
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
        replan_identico: false,
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
        replan_identico: false,
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
/// ronda y aprobando el gate del plan cuando frena.
fn correr_con(
    db: &EstadoDb,
    repo: &RepositorioSqlite,
    m: &dyn ClienteLlm,
    rec: Option<&entropia_agent::recuperacion::Recuperador>,
    dir: &std::path::Path,
    id: &str,
) -> Value {
    let mut out = procesar(db, repo, m, rec, dir, json!({"op":"get","job_id":id})).unwrap();
    for _ in 0..60 {
        match out["job"]["status"].as_str() {
            Some("done") => return out,
            Some("awaiting_human") => out = destrabar(db, repo, m, rec, dir, id, &out),
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
        replan_identico: false,
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
        replan_identico: false,
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

/// Eventos de un tipo en un snapshot.
fn eventos<'a>(snapshot: &'a Value, kind: &str) -> Vec<&'a Value> {
    snapshot["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == kind)
        .collect()
}

#[test]
fn un_limite_de_recuperacion_por_encima_del_techo_se_ajusta_sin_descartar_el_plan() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("techo-plan-artifacts");
    // El modelo pide más de lo que la recuperación híbrida entrega: el plan
    // es válido en todo lo demás y no hay motivo para tirarlo.
    let m = PlanPropio {
        base: modelo(false, false),
        plan: json!({"queries":["huelga"],"bibliography_queries":[],"retrieval_limit":50}),
        replan: json!({"queries":["huelga","asamblea"],"bibliography_queries":[],"retrieval_limit":50}),
    };
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    let planificado = step(&db, &repo, &m, &dir, &id);

    let plan = artefacto(&planificado, "plan");
    assert_eq!(plan["queries"], json!(["huelga"]), "{plan}");
    assert_eq!(plan["retrieval_limit"], 16, "{plan}");
    let ajustes = eventos(&planificado, "plan_adjusted");
    assert_eq!(ajustes.len(), 1, "{ajustes:?}");
    assert_eq!(
        ajustes[0]["payload"],
        json!({"field":"retrieval_limit","requested":50,"applied":16})
    );
    // Un ajuste no es una degradación: no hay plan de reemplazo que declarar.
    assert!(
        eventos(&planificado, "role_warning").is_empty(),
        "{:?}",
        eventos(&planificado, "role_warning")
    );

    // La replanificación pasa por el mismo ajuste en vez de descartarse.
    let replanificado = answer_round(&db, &repo, &m, &dir, &id);
    let plan = artefacto(&replanificado, "plan");
    assert_eq!(plan["queries"], json!(["huelga", "asamblea"]), "{plan}");
    assert_eq!(plan["retrieval_limit"], 16, "{plan}");
    assert_eq!(eventos(&replanificado, "plan_adjusted").len(), 2);
    assert!(
        eventos(&replanificado, "role_warning").is_empty(),
        "{:?}",
        eventos(&replanificado, "role_warning")
    );
}

#[test]
fn revisar_un_plan_con_limite_por_encima_del_techo_se_rechaza_con_el_rango_valido() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("techo-revise-artifacts");
    let m = modelo(false, false);
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    prepare_plan(&db, &repo, &m, &dir, &id);
    let snapshot = procesar(&db, &repo, &m, None, &dir, json!({"op":"get","job_id":id})).unwrap();
    let plan = snapshot["artifacts"]
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
    // Una revisión humana no se corrige en silencio: se rechaza y se dice por qué.
    let error = procesar(&db,&repo,&m,None,&dir,json!({"op":"revise","job_id":id,"artifact_id":plan,"content":{"queries":["paro"],"bibliography_queries":[],"retrieval_limit":17}})).unwrap_err();
    assert!(error.contains("1..16"), "{error}");
}

#[test]
fn el_plan_de_reemplazo_recupera_hasta_el_techo() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("techo-fallback-artifacts");
    // Sin consultas el plan es estructuralmente inválido: ahí sí se reemplaza.
    let vacio = json!({"queries":[],"bibliography_queries":[],"retrieval_limit":10});
    let m = PlanPropio {
        base: modelo(false, false),
        plan: vacio.clone(),
        replan: vacio,
    };
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    let planificado = step(&db, &repo, &m, &dir, &id);

    let plan = artefacto(&planificado, "plan");
    assert_eq!(plan["queries"], json!(["¿Hubo huelga?"]), "{plan}");
    assert_eq!(plan["retrieval_limit"], 16, "{plan}");
    assert!(eventos(&planificado, "role_warning")
        .iter()
        .any(|e| e["payload"]["error"]
            .as_str()
            .unwrap()
            .contains("plan incompleto")));
}

#[test]
fn sin_recuperador_el_techo_vale_por_consulta_y_no_por_coleccion() {
    let path = common::crear_corpus_sintetico();
    // Más coincidencias que el techo en cada una de las dos colecciones.
    common::agregar_chunks(&path, "c-conflicto", 12, "huelga general en el puerto");
    common::agregar_chunks(&path, "c-volantes", 12, "huelga general en el puerto");
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("techo-lexico-artifacts");
    let plan = json!({"queries":["huelga"],"bibliography_queries":[],"retrieval_limit":16});
    let m = PlanPropio {
        base: modelo(false, false),
        plan: plan.clone(),
        replan: plan,
    };
    let s = procesar(&db,&repo,&m,None,&dir,json!({"op":"create","question":"¿Hubo huelga?","project":"p","collection_ids":["c-conflicto","c-volantes"],"max_llm_calls":30,"max_cost":1.0})).unwrap();
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    let out = correr_con(&db, &repo, &m, None, &dir, &id);

    let consultas = eventos(&out, "query");
    assert!(!consultas.is_empty());
    for c in &consultas {
        assert_eq!(c["payload"]["pipeline"], "lexica", "{c}");
        assert!(
            c["payload"]["retrieved"].as_u64().unwrap() <= 16,
            "la consulta recuperó más que el techo: {c}"
        );
    }
    // El techo se alcanza: hay coincidencias de sobra en el recorte.
    assert!(
        consultas.iter().any(|c| c["payload"]["retrieved"] == 16),
        "{consultas:?}"
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
        replan_identico: false,
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
            out = destrabar(&db, &repo, &m, None, &dir, &id, &out);
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
    prepare_plan(&db, &repo, &m, &dir, &id);
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

#[test]
fn la_fecha_del_documento_viaja_desde_el_titulo_hasta_la_cita() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("fechas-artifacts");
    let m = modelo(false, false);
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    let out = correr(&db, &repo, &m, &dir, &id);

    // 1. La evidencia recuperada trae la fecha derivada del título del item.
    let evidencia = artefacto(&out, "archive")["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "chunk-1")
        .unwrap()
        .clone();
    assert_eq!(evidencia["document_date"]["iso"], "1965-03-17");
    assert_eq!(evidencia["document_date"]["precision"], "day");

    // 2. Y quedó asentada sobre la fuente en el ledger.
    let ledger_id = artefacto(&out, "archive")["claims"][0]["ledger_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ledger = entropia_agent::dominio::Ledger::nuevo(&db);
    let (evidencia_ledger, _, _) = ledger.evidencias_del_claim(&ledger_id)[0].clone();
    let (fecha, precision, confianza, derivacion) = ledger
        .metadata_temporal(&evidencia_ledger.source_id)
        .expect("la fuente tiene que declarar su fecha");
    assert_eq!(fecha, "1965-03-17");
    assert_eq!(precision, "day");
    assert!(confianza > 0.0);
    assert!(!derivacion.is_empty());

    // 3. Y el informe la declara en la referencia.
    let md = std::fs::read_to_string(dir.join(&id).join("report.md")).unwrap();
    assert!(
        md.contains("· 1965-03-17 · fragmento chunk-1"),
        "la referencia tiene que declarar la fecha del documento:\n{md}"
    );
}

#[test]
fn una_fecha_imprecisa_se_declara_imprecisa() {
    // El render recorta a la precisión y la nombra: un año derivado no puede
    // leerse como un día exacto.
    let referencia = json!({
        "n": 1,
        "chunk_id": "chunk-9",
        "title": "IMG_2991",
        "date": "1961",
        "date_precision": "year",
        "start": 0,
        "end": 10
    });
    let artefacto = json!({
        "report": {"title":"T","sections":[],"references":[referencia]},
        "coverage": {"collections":[]}
    });
    let md = entropia_agent::informe_render::render(&artefacto);
    assert!(md.contains("1961 (año)"), "{md}");
}

#[test]
fn la_ronda_no_se_puede_cerrar_aprobando_el_gate() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("gate-ronda-artifacts");
    let m = modelo(false, false);
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    let abierta = step(&db, &repo, &m, &dir, &id);
    assert_eq!(abierta["job"]["status"], "awaiting_human");

    // Aprobar el gate de la ronda dejaba el job en `running` sobre una etapa
    // que no puede avanzar: cualquier conductor que reintente mientras siga
    // corriendo gira en vacío para siempre.
    let gate = abierta["gates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["status"] == "pending")
        .unwrap()["id"]
        .clone();
    let error = procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"decision","job_id":id,"gate_id":gate,"approve":true}),
    )
    .expect_err("la ronda no se aprueba, se responde");
    assert!(error.contains("answer"), "{error}");
    assert_eq!(
        procesar(&db, &repo, &m, None, &dir, json!({"op":"get","job_id":id})).unwrap()["job"]
            ["status"],
        "awaiting_human"
    );
}

#[test]
fn una_ronda_abierta_estaciona_el_job_en_vez_de_girar_en_vacio() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let state = path.with_extension("giro-state.sqlite");
    let db = EstadoDb::abrir(state.to_str().unwrap()).unwrap();
    let dir = path.with_extension("giro-artifacts");
    let m = modelo(false, false);
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);

    // Se fuerza el estado que producía el cuelgue: gate aprobado a mano y job
    // corriendo con la ronda todavía sin responder.
    drop(db);
    {
        let conn = rusqlite::Connection::open(&state).unwrap();
        conn.execute(
            "UPDATE human_decisions SET decision='approved' WHERE job_id=?1 AND decision='pending'",
            [&id],
        )
        .unwrap();
    }
    let db = EstadoDb::abrir(state.to_str().unwrap()).unwrap();
    procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"resume","job_id":id}),
    )
    .unwrap();

    let out = procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"advance","job_id":id}),
    )
    .unwrap();
    assert_eq!(
        out["job"]["status"], "awaiting_human",
        "el job tiene que estacionarse, no quedar corriendo sin progreso"
    );
    assert!(out["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "clarification_pending"));
}

#[test]
fn el_artefacto_lleva_el_informe_renderizado_para_que_nadie_lo_rearme() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("markdown-artifacts");
    let m = modelo(false, false);
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    let out = correr(&db, &repo, &m, &dir, &id);

    // Un consumidor que rearma el documento desde `sections[].text` pierde la
    // cobertura, los fragmentos citados y «Fuentes citadas». El armado es del
    // motor: el artefacto lo lleva hecho.
    let markdown = artefacto(&out, "report")["markdown"]
        .as_str()
        .expect("el artefacto tiene que llevar el informe renderizado")
        .to_owned();
    assert!(
        markdown.contains("## Cobertura del recorte consultado"),
        "{markdown}"
    );
    assert!(markdown.contains("## Fuentes citadas"), "{markdown}");
    assert!(markdown.contains("> huelga general"), "{markdown}");
    assert!(markdown.contains("**Perfil de informe:**"), "{markdown}");

    // Y es exactamente lo que se escribe en disco: una sola fuente de armado.
    let en_disco = std::fs::read_to_string(dir.join(&id).join("report.md")).unwrap();
    assert_eq!(en_disco, markdown);
}

#[test]
fn el_titulo_lo_pone_el_investigador_y_la_pregunta_es_el_respaldo() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("titulo-artifacts");
    let m = modelo(false, false);

    // Con título propio: la pregunta puede ser larga y no sirve de nombre.
    let con = procesar(&db,&repo,&m,None,&dir,json!({"op":"create","title":"El conflicto del filet","question":"¿Cómo se organizó el conflicto gremial del SOIP entre marzo y abril de 1965?","project":"p","collection_ids":["c-conflicto"],"max_llm_calls":30})).unwrap();
    assert_eq!(con["job"]["title"], "El conflicto del filet");
    assert_eq!(
        con["job"]["question"],
        "¿Cómo se organizó el conflicto gremial del SOIP entre marzo y abril de 1965?"
    );

    // Sin título: la pregunta lo cubre, y el job nunca queda sin nombre.
    let sin = create(&db, &repo, &m, &dir);
    assert_eq!(sin["job"]["title"], "¿Hubo huelga?");

    // Un título en blanco no cuenta como título.
    let vacio = procesar(&db,&repo,&m,None,&dir,json!({"op":"create","title":"   ","question":"¿Hubo paro?","project":"p","collection_ids":["c-conflicto"],"max_llm_calls":30})).unwrap();
    assert_eq!(vacio["job"]["title"], "¿Hubo paro?");

    // Y viaja en el listado, que es donde el investigador lo lee.
    let listado = procesar(&db, &repo, &m, None, &dir, json!({"op":"list"})).unwrap();
    assert!(listado["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|j| j["title"] == "El conflicto del filet"));
}

#[test]
fn borrar_una_investigacion_se_lleva_todo_lo_que_colgaba_de_ella() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let state = path.with_extension("borrar-state.sqlite");
    let db = EstadoDb::abrir(state.to_str().unwrap()).unwrap();
    let dir = path.with_extension("borrar-artifacts");
    let m = modelo(false, false);
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    correr(&db, &repo, &m, &dir, &id);
    assert!(dir.join(&id).join("report.md").exists());

    let ledger_id = artefacto(
        &procesar(&db, &repo, &m, None, &dir, json!({"op":"get","job_id":id})).unwrap(),
        "archive",
    )["claims"][0]["ledger_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let borrado = procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"delete","job_id":id}),
    )
    .unwrap();
    assert_eq!(borrado["deleted"], id.as_str());

    // El job ya no existe ni se puede consultar.
    assert!(procesar(&db, &repo, &m, None, &dir, json!({"op":"get","job_id":id})).is_err());
    assert!(
        procesar(&db, &repo, &m, None, &dir, json!({"op":"list"})).unwrap()["jobs"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    // Ni sus claims en el ledger: una investigación a medio borrar es peor
    // que ninguna.
    let ledger = entropia_agent::dominio::Ledger::nuevo(&db);
    assert!(ledger.claim(&ledger_id).is_none());
    assert!(ledger.runs_del_claim(&ledger_id).is_empty());
    assert!(ledger.evidencias_del_claim(&ledger_id).is_empty());

    // Ni sus archivos: si quedaran, el próximo job con ese id leería un
    // informe ajeno.
    assert!(!dir.join(&id).exists());
}

#[test]
fn una_investigacion_corriendo_no_se_borra() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("borrar-viva-artifacts");
    let m = modelo(false, false);
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    assert_eq!(s["job"]["status"], "running");

    // El conductor podría estar a mitad de un paso y reescribir filas recién
    // borradas: primero se cancela.
    let error = procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"delete","job_id":id}),
    )
    .expect_err("un job corriendo no se borra");
    assert!(error.contains("Cancelá"), "{error}");

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
        json!({"op":"delete","job_id":id})
    )
    .is_ok());
}

#[test]
fn una_replanificacion_que_no_cambia_nada_queda_declarada() {
    // Visto en un archivo real: se respondieron cuatro preguntas de encuadre
    // —«solo el conflicto 1965», «priorizar la identidad», «prensa»— y el plan
    // volvió idéntico byte a byte, conservando la consulta «huelga 1966». El
    // encuadre no tuvo ningún efecto sobre la búsqueda y nadie lo dijo.
    //
    // Que el modelo no cambie el plan es su derecho. Aceptarlo en silencio no:
    // quien respondió esas preguntas queda creyendo que orientó la
    // investigación. La misma función ya avisa cuando la replanificación se va
    // de los límites; una que no cambia nada merece el mismo trato.
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
        replan_identico: true,
    };

    let id = create(&db, &repo, &m, &dir)["job"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    // Tres pasos llegan al plan; el cuarto abre la ronda que `answer_round`
    // responde y cierra con la replanificación. (`prepare_plan` no sirve acá:
    // ya responde la ronda por dentro.)
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    let planeado = step(&db, &repo, &m, &dir, &id);
    assert_eq!(artefacto(&planeado, "plan")["queries"], json!(["huelga"]));

    let cerrada = answer_round(&db, &repo, &m, &dir, &id);

    assert!(
        cerrada["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "role_warning"
                && e["payload"]["error"]
                    .as_str()
                    .is_some_and(|t| t.contains("no cambió el plan"))),
        "una replanificación sin efecto tiene que quedar declarada: {}",
        cerrada["events"]
    );

    // Y no deja una versión nueva idéntica a la anterior: un artefacto que
    // repite al que ya estaba no registra nada, sólo ensucia la historia.
    let planes: Vec<_> = cerrada["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["kind"] == "plan" && a["obsolete"] == false)
        .collect();
    assert_eq!(
        planes.len(),
        1,
        "el plan no cambió, así que no hay una segunda versión que guardar: {planes:?}"
    );
}

/// Consultas numeradas para un plan propio del modelo.
fn consultas_numeradas(n: usize) -> Vec<String> {
    (1..=n).map(|i| format!("huelga variante {i}")).collect()
}

#[test]
fn trayectorias_conserva_un_plan_de_veinticinco_consultas() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("tope-trayectorias-artifacts");
    // Una consulta por variante de nombre, cargo y organización de cada actor:
    // el perfil de trayectorias necesita más consultas que el general.
    let plan =
        json!({"queries":consultas_numeradas(25),"bibliography_queries":[],"retrieval_limit":10});
    let m = PlanPropio {
        base: modelo(false, false),
        plan: plan.clone(),
        replan: plan,
    };
    let s = procesar(&db,&repo,&m,None,&dir,json!({"op":"create","question":"¿Hubo huelga?","project":"p","collection_ids":["c-conflicto"],"max_llm_calls":30,"max_cost":1.0,"modalidad":"trayectorias"})).unwrap();
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    let planificado = step(&db, &repo, &m, &dir, &id);

    let plan = artefacto(&planificado, "plan");
    assert_eq!(plan["queries"], json!(consultas_numeradas(25)), "{plan}");
    assert!(
        !eventos(&planificado, "plan_adjusted")
            .iter()
            .any(|e| e["payload"]["field"] == "queries"),
        "{:?}",
        eventos(&planificado, "plan_adjusted")
    );
}

#[test]
fn un_plan_general_por_encima_del_tope_conserva_las_primeras_consultas() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("tope-general-artifacts");
    // El prompt pide las consultas de mayor a menor prioridad: recortar la
    // cola descarta las menos importantes sin tirar el plan entero.
    let plan =
        json!({"queries":consultas_numeradas(25),"bibliography_queries":[],"retrieval_limit":10});
    let m = PlanPropio {
        base: modelo(false, false),
        plan: plan.clone(),
        replan: plan,
    };
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    let planificado = step(&db, &repo, &m, &dir, &id);

    let plan = artefacto(&planificado, "plan");
    assert_eq!(plan["queries"], json!(consultas_numeradas(20)), "{plan}");
    let ajustes: Vec<&Value> = eventos(&planificado, "plan_adjusted")
        .into_iter()
        .filter(|e| e["payload"]["field"] == "queries")
        .collect();
    assert_eq!(ajustes.len(), 1, "{ajustes:?}");
    assert_eq!(
        ajustes[0]["payload"],
        json!({"field":"queries","requested":25,"applied":20})
    );
    // Un recorte no es un plan de reemplazo.
    assert!(
        eventos(&planificado, "role_warning").is_empty(),
        "{:?}",
        eventos(&planificado, "role_warning")
    );
}

#[test]
fn revisar_un_plan_por_encima_del_tope_de_consultas_se_rechaza_con_el_rango_valido() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("tope-revise-artifacts");
    let m = modelo(false, false);
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    prepare_plan(&db, &repo, &m, &dir, &id);
    let snapshot = procesar(&db, &repo, &m, None, &dir, json!({"op":"get","job_id":id})).unwrap();
    let plan = snapshot["artifacts"]
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
    // Una revisión humana no se recorta en silencio: se rechaza con el rango.
    let error = procesar(&db,&repo,&m,None,&dir,json!({"op":"revise","job_id":id,"artifact_id":plan,"content":{"queries":consultas_numeradas(21),"bibliography_queries":[],"retrieval_limit":5}})).unwrap_err();
    assert!(error.contains("entre 1 y 20 consultas"), "{error}");
}

/// Línea del informe que declara las búsquedas en el corpus y sus llamadas.
fn linea_busquedas(consultas: usize, embeddings: usize, rerank: usize) -> String {
    format!("Búsquedas en el corpus: {consultas} ({embeddings} llamadas de embeddings, {rerank} de rerank; no se descuentan del presupuesto de llamadas al modelo).")
}

#[test]
fn las_llamadas_de_recuperacion_se_declaran_por_consulta_y_en_el_informe() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let m = modelo(false, false);

    // Pipeline híbrido: cada consulta embebe una vez y reranquea una vez,
    // porque el recorte tiene fragmentos que entran al rerank.
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("llamadas-hibrida-artifacts");
    let rec = entropia_agent::recuperacion::Recuperador::con_clientes(
        Box::new(EmbedFijo),
        Box::new(RerankIdentidad),
    );
    let s = procesar(&db,&repo,&m,Some(&rec),&dir,json!({"op":"create","question":"¿Hubo huelga?","project":"p","collection_ids":["c-conflicto"],"max_llm_calls":30,"max_cost":1.0})).unwrap();
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    let out = correr_con(&db, &repo, &m, Some(&rec), &dir, &id);
    let consultas = eventos(&out, "query");
    assert!(!consultas.is_empty());
    for c in &consultas {
        assert_eq!(
            c["payload"]["calls"],
            json!({"embeddings":1,"rerank":1}),
            "{c}"
        );
    }
    let n = consultas.len();
    let md = std::fs::read_to_string(dir.join(&id).join("report.md")).unwrap();
    assert!(md.contains(&linea_busquedas(n, n, n)), "{md}");

    // Búsqueda léxica sola: ninguna llamada externa que declarar.
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("llamadas-lexica-artifacts");
    let s = create(&db, &repo, &m, &dir);
    let id = s["job"]["id"].as_str().unwrap().to_owned();
    let out = correr(&db, &repo, &m, &dir, &id);
    let consultas = eventos(&out, "query");
    assert!(!consultas.is_empty());
    for c in &consultas {
        assert_eq!(
            c["payload"]["calls"],
            json!({"embeddings":0,"rerank":0}),
            "{c}"
        );
    }
    let md = std::fs::read_to_string(dir.join(&id).join("report.md")).unwrap();
    assert!(md.contains(&linea_busquedas(consultas.len(), 0, 0)), "{md}");
}

/// Gate pendiente de un snapshot, si lo hay.
fn gate_pendiente(snapshot: &Value) -> Option<Value> {
    snapshot["gates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["status"] == "pending")
        .cloned()
}

/// Id del artefacto vigente de un tipo.
fn id_vigente(snapshot: &Value, kind: &str) -> Value {
    snapshot["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .rfind(|a| a["kind"] == kind && a["obsolete"] == false)
        .unwrap_or_else(|| panic!("falta el artefacto {kind}"))["id"]
        .clone()
}

#[test]
fn cerrar_la_ronda_deja_el_plan_final_esperando_un_gate() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("gate-plan-artifacts");
    let m = modelo(false, false);
    let id = create(&db, &repo, &m, &dir)["job"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    let cerrada = answer_round(&db, &repo, &m, &dir, &id);

    // Las búsquedas quedan a la vista del historiador antes de ir al corpus.
    assert_eq!(
        cerrada["job"]["status"], "awaiting_human",
        "{}",
        cerrada["job"]
    );
    let gate = gate_pendiente(&cerrada).expect("el plan final tiene que esperar un gate");
    assert_eq!(gate["kind"], "plan", "{gate}");
    assert_eq!(gate["artifact_id"], id_vigente(&cerrada, "plan"), "{gate}");

    // Sin decisión no se gasta presupuesto de búsqueda ni de lectura.
    let llamadas = cerrada["job"]["llm_calls"].clone();
    assert!(procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"advance","job_id":id})
    )
    .is_err());
    let despues = procesar(&db, &repo, &m, None, &dir, json!({"op":"get","job_id":id})).unwrap();
    assert_eq!(despues["job"]["status"], "awaiting_human");
    assert_eq!(despues["job"]["llm_calls"], llamadas);
    assert!(entradas(&despues, "asistente_archivo").is_empty());
}

#[test]
fn aprobar_el_gate_del_plan_deja_seguir_la_investigacion_hasta_el_informe() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("gate-aprobado-artifacts");
    let m = modelo(false, false);
    let id = create(&db, &repo, &m, &dir)["job"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    let cerrada = answer_round(&db, &repo, &m, &dir, &id);
    let gate = gate_pendiente(&cerrada).expect("el plan final tiene que esperar un gate");

    let aprobada = procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"decision","job_id":id,"gate_id":gate["id"],"approve":true}),
    )
    .unwrap();
    assert_eq!(aprobada["job"]["status"], "running", "{}", aprobada["job"]);
    assert!(gate_pendiente(&aprobada).is_none());

    // Aprobado el plan, nada más frena la investigación hasta el informe.
    let mut out = aprobada;
    for _ in 0..20 {
        if out["job"]["status"] != "running" {
            break;
        }
        out = step(&db, &repo, &m, &dir, &id);
    }
    assert_eq!(out["job"]["status"], "done", "{}", out["job"]);
    assert!(!entradas(&out, "asistente_archivo").is_empty());
    assert!(dir.join(&id).join("report.json").exists());
}

#[test]
fn editar_las_busquedas_en_el_gate_las_aprueba_sin_reabrir_la_ronda() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("gate-editado-artifacts");
    let m = modelo(false, false);
    let id = create(&db, &repo, &m, &dir)["job"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    let cerrada = answer_round(&db, &repo, &m, &dir, &id);
    let gate = gate_pendiente(&cerrada).expect("el plan final tiene que esperar un gate");

    let editado = procesar(&db,&repo,&m,None,&dir,json!({"op":"revise","job_id":id,"artifact_id":gate["artifact_id"],"content":{"queries":["huelga general","paro"],"bibliography_queries":[],"retrieval_limit":5}})).unwrap();

    // Editar es aprobar: no queda gate pendiente y la decisión queda asentada.
    assert_eq!(editado["job"]["status"], "running", "{}", editado["job"]);
    assert!(gate_pendiente(&editado).is_none(), "{}", editado["gates"]);
    let plan_editado = id_vigente(&editado, "plan");
    assert!(
        editado["gates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["kind"] == "plan"
                && g["artifact_id"] == plan_editado
                && g["status"] == "approved"),
        "{}",
        editado["gates"]
    );
    // La ronda respondida se conserva: la ejecución retoma en el archivo.
    assert_eq!(editado["job"]["phase"], "execution");
    assert!(artefacto(&editado, "clarification_round")["answers"].is_array());

    let out = correr(&db, &repo, &m, &dir, &id);
    let consultas: Vec<&str> = eventos(&out, "query")
        .iter()
        .map(|e| e["payload"]["query"].as_str().unwrap())
        .collect();
    assert_eq!(consultas, vec!["huelga general", "paro"]);
    assert_eq!(
        eventos(&out, "clarification_requested").len(),
        1,
        "la ronda no se vuelve a preguntar"
    );
    let md = std::fs::read_to_string(dir.join(&id).join("report.md")).unwrap();
    assert!(
        md.contains("## Encuadre acordado con el investigador"),
        "{md}"
    );
    assert!(
        md.contains("1965-1966, conflicto gremial, actas de asamblea"),
        "{md}"
    );
}

#[test]
fn responder_la_ronda_con_el_diseno_editado_lo_versiona_y_el_replan_lo_recibe() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let state = path.with_extension("diseno-ronda-state.sqlite");
    let db = EstadoDb::abrir(state.to_str().unwrap()).unwrap();
    let dir = path.with_extension("diseno-ronda-artifacts");
    let m = modelo(false, false);
    let id = create(&db, &repo, &m, &dir)["job"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    let abierta = step(&db, &repo, &m, &dir, &id);
    let answers = respuestas(&round_questions(&abierta));
    let diseno_previo = id_vigente(&abierta, "design");

    // Un diseño con campos vacíos se rechaza y la ronda sigue abierta.
    let error = procesar(&db,&repo,&m,None,&dir,json!({"op":"answer","job_id":id,"answers":answers,"design":{"hypothesis":"  ","scope":"","closing_criteria":[]}})).unwrap_err();
    assert!(error.contains("Diseño incompleto"), "{error}");
    let sigue = procesar(&db, &repo, &m, None, &dir, json!({"op":"get","job_id":id})).unwrap();
    assert_eq!(sigue["job"]["status"], "awaiting_human");
    assert_eq!(
        gate_pendiente(&sigue).expect("la ronda sigue pendiente")["kind"],
        "clarification_round"
    );
    assert_eq!(id_vigente(&sigue, "design"), diseno_previo);

    // Un diseño válido queda como versión nueva, colgada de la anterior.
    let editado = json!({"hypothesis":"La huelga de 1965 respondió a despidos","scope":"SOIP, 1965-1966","closing_criteria":["Actas de asamblea revisadas","Volantes fechados"]});
    let respondida = procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"answer","job_id":id,"answers":answers,"design":editado}),
    )
    .unwrap();
    assert_eq!(artefacto(&respondida, "design"), editado);
    let diseno_nuevo = id_vigente(&respondida, "design");
    assert_ne!(diseno_nuevo, diseno_previo);
    assert_eq!(eventos(&respondida, "design_edited").len(), 1);
    {
        let conn = rusqlite::Connection::open(&state).unwrap();
        let padre: String = conn
            .query_row(
                "SELECT padre FROM artifacts WHERE id=?1",
                [diseno_nuevo.as_str().unwrap()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(padre, diseno_previo.as_str().unwrap());
    }

    // El replan trabaja sobre el diseño editado y cierra en el gate del plan.
    let cerrada = step(&db, &repo, &m, &dir, &id);
    let replan = entradas(&cerrada, "investigador_principal")
        .into_iter()
        .rfind(|e| e["contract"].as_str().unwrap().contains("replanificá"))
        .expect("tiene que haber una replanificación");
    assert_eq!(replan["data"]["design"], editado);
    assert_eq!(
        gate_pendiente(&cerrada).expect("el plan final espera su gate")["kind"],
        "plan"
    );
}

#[test]
fn una_replanificacion_que_no_cambia_el_plan_igual_abre_el_gate() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("gate-replan-identico-artifacts");
    let m = Model {
        replan_identico: true,
        ..modelo(false, false)
    };
    let id = create(&db, &repo, &m, &dir)["job"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    let cerrada = answer_round(&db, &repo, &m, &dir, &id);

    // Que las respuestas no cambien el plan no lo vuelve aprobado: el
    // historiador igual tiene que ver las búsquedas antes de ir al corpus.
    assert_eq!(
        cerrada["job"]["status"], "awaiting_human",
        "{}",
        cerrada["job"]
    );
    let gate = gate_pendiente(&cerrada).expect("el plan final tiene que esperar un gate");
    assert_eq!(gate["kind"], "plan", "{gate}");
    assert_eq!(gate["artifact_id"], id_vigente(&cerrada, "plan"), "{gate}");
    assert_eq!(
        artefacto(&cerrada, "plan")["queries"],
        json!(["huelga"]),
        "el plan vigente es el de antes de la ronda"
    );
    // El desktop avisa que el plan no cambió leyendo este código, no la prosa
    // del mensaje: el código es el contrato.
    assert!(
        cerrada["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "role_warning" && e["payload"]["code"] == "plan_unchanged"),
        "falta el aviso con código plan_unchanged: {}",
        cerrada["events"]
    );
}

#[test]
fn editar_el_diseno_fuera_de_la_ronda_rehace_el_plan_y_vuelve_a_preguntar() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("diseno-revisado-artifacts");
    let m = modelo(false, false);
    let id = create(&db, &repo, &m, &dir)["job"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    let en_gate = answer_round(&db, &repo, &m, &dir, &id);
    assert_eq!(gate_pendiente(&en_gate).unwrap()["kind"], "plan");

    let editado = json!({"hypothesis":"La huelga de 1965 respondió a despidos","scope":"SOIP, 1965-1966","closing_criteria":["Actas de asamblea revisadas"]});
    let revisado = procesar(&db,&repo,&m,None,&dir,json!({"op":"revise","job_id":id,"artifact_id":id_vigente(&en_gate, "design"),"content":editado})).unwrap();

    // Editar es aprobar, pero las preguntas dependen del diseño: se vuelve al plan.
    assert_eq!(revisado["job"]["status"], "running", "{}", revisado["job"]);
    assert_eq!(revisado["job"]["phase"], "plan");
    assert!(gate_pendiente(&revisado).is_none(), "{}", revisado["gates"]);
    assert!(revisado["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["kind"] == "clarification_round" || a["kind"] == "clarification")
        .all(|a| a["obsolete"] == true));

    // El plan se rehace sobre el diseño editado.
    let replaneado = step(&db, &repo, &m, &dir, &id);
    let plan = entradas(&replaneado, "investigador_principal")
        .into_iter()
        .rfind(|e| {
            let c = e["contract"].as_str().unwrap();
            c.contains("{queries:") && !c.contains("replanificá")
        })
        .expect("tiene que haber un plan nuevo");
    assert_eq!(plan["data"], editado);

    // La ronda se vuelve a preguntar y el job termina otra vez en el gate del plan.
    let cerrada = answer_round(&db, &repo, &m, &dir, &id);
    assert_eq!(eventos(&cerrada, "clarification_requested").len(), 2);
    assert_eq!(
        cerrada["job"]["status"], "awaiting_human",
        "{}",
        cerrada["job"]
    );
    let gate = gate_pendiente(&cerrada).expect("el plan final tiene que esperar un gate");
    assert_eq!(gate["kind"], "plan", "{gate}");
    assert_eq!(gate["artifact_id"], id_vigente(&cerrada, "plan"), "{gate}");
}

#[test]
fn editar_el_plan_con_la_ronda_abierta_no_la_saltea() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("plan-ronda-abierta-artifacts");
    let m = modelo(false, false);
    let id = create(&db, &repo, &m, &dir)["job"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    step(&db, &repo, &m, &dir, &id);
    let abierta = step(&db, &repo, &m, &dir, &id);

    let editado = json!({"queries":["paro"],"bibliography_queries":[],"retrieval_limit":5});
    let revisado = procesar(&db,&repo,&m,None,&dir,json!({"op":"revise","job_id":id,"artifact_id":id_vigente(&abierta, "plan"),"content":editado})).unwrap();

    // La ronda sigue abierta: el job la espera en vez de quedar varado en el
    // archivo con un gate que ya nadie puede resolver.
    assert_eq!(
        revisado["job"]["status"], "awaiting_human",
        "{}",
        revisado["job"]
    );
    assert_eq!(revisado["job"]["phase"], "clarification");
    assert_eq!(
        gate_pendiente(&revisado).expect("la ronda sigue pendiente")["kind"],
        "clarification_round"
    );

    // Respondida, se replanifica sobre el plan editado y se cierra en el gate.
    let answers = respuestas(&round_questions(&abierta));
    let respondida = procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"answer","job_id":id,"answers":answers}),
    )
    .unwrap();
    assert_eq!(respondida["job"]["status"], "running");
    let cerrada = step(&db, &repo, &m, &dir, &id);
    let replan = entradas(&cerrada, "investigador_principal")
        .into_iter()
        .rfind(|e| e["contract"].as_str().unwrap().contains("replanificá"))
        .expect("tiene que haber una replanificación");
    assert_eq!(replan["data"]["plan"], editado);
    assert_eq!(
        gate_pendiente(&cerrada).expect("el plan final espera su gate")["kind"],
        "plan"
    );
}

/// Redactor guionado: entrega un informe de dos secciones y reescribe una
/// sección citando los `claim_ids` que el test le indique. El resto de los
/// roles los responde `Model`.
struct Redactor {
    base: Model,
    /// `claim_ids` que devuelve la reescritura de una sección.
    claim_ids_reescritura: Value,
}
impl ClienteLlm for Redactor {
    fn modelo(&self) -> &str {
        self.base.modelo()
    }
    fn ultimo_costo(&self) -> Option<f64> {
        self.base.ultimo_costo()
    }
    fn turno_agente(&self, m: &[Value], t: &[Value]) -> Result<TurnoAgente, String> {
        let s = m[0]["content"].as_str().unwrap();
        if s.contains("Rol: asistente_redaccion.") && s.contains("reescribí una sola sección") {
            Ok(TurnoAgente::Texto(
                json!({"title":"Contexto reescrito","text":"El gremio llegó a la huelga tras un conflicto largo","claim_ids":self.claim_ids_reescritura})
                    .to_string(),
            ))
        } else if s.contains("Rol: asistente_redaccion.") {
            Ok(TurnoAgente::Texto(
                json!({"title":"Huelga","sections":[
                    {"title":"Hechos","text":"Hubo una huelga","claim_ids":["c1"]},
                    {"title":"Contexto","text":"El gremio venía de un conflicto largo","claim_ids":[]}
                ]})
                .to_string(),
            ))
        } else {
            self.base.turno_agente(m, t)
        }
    }
}

fn redactor() -> Redactor {
    redactor_que_cita(json!(["c1"]))
}

fn redactor_que_cita(claim_ids: Value) -> Redactor {
    Redactor {
        base: modelo(false, false),
        claim_ids_reescritura: claim_ids,
    }
}

/// Secciones del informe vigente en un snapshot.
fn secciones(snapshot: &Value) -> Vec<Value> {
    artefacto(snapshot, "report")["report"]["sections"]
        .as_array()
        .expect("el informe tiene que traer secciones")
        .clone()
}

#[test]
fn al_redactar_cada_seccion_tiene_id_version_y_origen() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("secciones-id-artifacts");
    let m = redactor();
    let id = create(&db, &repo, &m, &dir)["job"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let out = correr_con(&db, &repo, &m, None, &dir, &id);

    let secciones = secciones(&out);
    assert_eq!(secciones.len(), 2, "{secciones:?}");
    for (i, s) in secciones.iter().enumerate() {
        assert_eq!(s["id"], format!("s{}", i + 1), "{s}");
        assert_eq!(s["version"], 1, "{s}");
        assert_eq!(s["origen"], "redactor", "{s}");
        assert!(s["indicacion"].is_null(), "{s}");
    }
}

/// Artefactos `report` de un snapshot, en orden de escritura.
fn informes(snapshot: &Value) -> Vec<Value> {
    snapshot["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["kind"] == "report")
        .cloned()
        .collect()
}

/// Cada número citado tiene su referencia y la numeración es correlativa
/// desde 1.
fn numeracion_consistente(informe: &Value) {
    let usados: std::collections::BTreeSet<i64> = informe["report"]["sections"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["quotes"].as_array().unwrap().iter())
        .map(|q| q["n"].as_i64().unwrap())
        .collect();
    let declarados: std::collections::BTreeSet<i64> = informe["report"]["references"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["n"].as_i64().unwrap())
        .collect();
    assert_eq!(usados, declarados, "hay números citados sin referencia");
    assert_eq!(
        declarados.iter().copied().collect::<Vec<_>>(),
        (1..=declarados.len() as i64).collect::<Vec<_>>(),
        "la numeración tiene que ser correlativa desde 1"
    );
}

const AVISO_EDICION: &str =
    "Sección editada por el historiador: el texto no pasó por la verificación";

#[test]
fn editar_una_seccion_de_una_investigacion_cerrada_versiona_el_informe() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("editar-seccion-artifacts");
    let m = redactor();
    let id = create(&db, &repo, &m, &dir)["job"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let cerrada = correr_con(&db, &repo, &m, None, &dir, &id);
    let previo = artefacto(&cerrada, "report");
    let previo_id = id_vigente(&cerrada, "report");
    let llamadas = cerrada["job"]["llm_calls"].clone();
    let texto = "El conflicto venía de antes: ver el tramo [9] del expediente.";

    let editada = procesar(&db,&repo,&m,None,&dir,json!({"op":"edit_section","job_id":id,"section_id":"s2","title":"Contexto gremial","text":texto})).unwrap();

    // La investigación sigue cerrada con su informe, y editar no gasta modelo.
    assert_eq!(editada["job"]["status"], "done");
    assert_eq!(editada["job"]["close_reason"], "completed");
    assert_eq!(editada["job"]["llm_calls"], llamadas);

    // Versión nueva del informe; la anterior queda intacta.
    let versiones = informes(&editada);
    assert_eq!(versiones.len(), 2, "{versiones:?}");
    assert_eq!(versiones[0]["id"], previo_id);
    assert_eq!(versiones[0]["content"], previo);
    assert_eq!(versiones[1]["version"], 2);
    let nuevo = artefacto(&editada, "report");

    // Solo cambia la sección tocada, y conserva sus claims.
    let antes = previo["report"]["sections"].as_array().unwrap();
    let despues = nuevo["report"]["sections"].as_array().unwrap();
    assert_eq!(despues.len(), 2);
    assert_eq!(despues[0], antes[0], "la otra sección no puede cambiar");
    let s2 = &despues[1];
    assert_eq!(s2["id"], "s2");
    assert_eq!(s2["title"], "Contexto gremial");
    assert_eq!(s2["text"], texto);
    assert_eq!(s2["origen"], "historiador");
    assert_eq!(s2["version"], 2);
    assert_eq!(s2["claim_ids"], antes[1]["claim_ids"]);

    // El resto del contenido viaja igual.
    for clave in [
        "coverage",
        "coverage_warning",
        "archive_limitations",
        "dropped_claims",
        "role_warnings",
        "verification",
        "bibliography",
        "clarification",
        "profile",
        "retrieval_calls",
    ] {
        assert_eq!(nuevo[clave], previo[clave], "{clave}");
    }

    // Las citas se rearman sobre el informe completo.
    numeracion_consistente(&nuevo);
    assert_eq!(
        nuevo["report"]["references"],
        previo["report"]["references"]
    );

    // El markdown declara la edición, solo en la sección editada, y el
    // corchete del historiador no se cuenta como cita.
    let md = nuevo["markdown"].as_str().unwrap();
    assert_eq!(md.matches(AVISO_EDICION).count(), 1, "{md}");
    assert!(md.contains("## Contexto gremial"), "{md}");
    assert!(
        entropia_agent::informe_render::citas_sin_referencia(md).is_empty(),
        "{md}"
    );

    // Una sola fuente de armado: el disco lleva la versión nueva.
    assert_eq!(
        std::fs::read_to_string(dir.join(&id).join("report.md")).unwrap(),
        md
    );
    let en_disco: Value =
        serde_json::from_slice(&std::fs::read(dir.join(&id).join("report.json")).unwrap()).unwrap();
    assert_eq!(en_disco, nuevo);

    let evento = eventos(&editada, "section_edited");
    assert_eq!(evento.len(), 1, "{evento:?}");
    assert_eq!(evento[0]["payload"]["section_id"], "s2");
    assert_eq!(evento[0]["payload"]["version"], 2);
}

#[test]
fn editar_una_seccion_rechaza_la_inexistente_el_texto_vacio_y_la_investigacion_sin_informe() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("editar-rechazos-artifacts");
    let m = redactor();
    let editar = |id: &str, seccion: &str, texto: &str| {
        procesar(
            &db,
            &repo,
            &m,
            None,
            &dir,
            json!({"op":"edit_section","job_id":id,"section_id":seccion,"text":texto}),
        )
    };
    let id = create(&db, &repo, &m, &dir)["job"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    correr_con(&db, &repo, &m, None, &dir, &id);

    let error = editar(&id, "s9", "Texto nuevo").unwrap_err();
    assert!(error.contains("no existe"), "{error}");
    let error = editar(&id, "s1", "   ").unwrap_err();
    assert!(error.contains("vacío"), "{error}");
    // Un rechazo no deja rastro: ni versión nueva ni evento.
    let intacta = procesar(&db, &repo, &m, None, &dir, json!({"op":"get","job_id":id})).unwrap();
    assert_eq!(informes(&intacta).len(), 1);
    assert!(eventos(&intacta, "section_edited").is_empty());

    // Sin informe no hay nada que editar: ni en curso ni cancelada.
    let otra = create(&db, &repo, &m, &dir)["job"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let error = editar(&otra, "s1", "Texto nuevo").unwrap_err();
    assert!(error.contains("no tiene informe"), "{error}");
    procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"cancel","job_id":otra}),
    )
    .unwrap();
    let error = editar(&otra, "s1", "Texto nuevo").unwrap_err();
    assert!(error.contains("no tiene informe"), "{error}");
}

#[test]
fn reescribir_una_seccion_llama_una_vez_al_redactor_y_rechaza_claims_no_verificados() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("reescribir-seccion-artifacts");
    let m = redactor();
    let id = create(&db, &repo, &m, &dir)["job"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let cerrada = correr_con(&db, &repo, &m, None, &dir, &id);
    let previo = artefacto(&cerrada, "report");
    let llamadas = cerrada["job"]["llm_calls"].as_i64().unwrap();
    let indicacion = "Situalo en la huelga de marzo";

    let reescrita = procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"rewrite_section","job_id":id,"section_id":"s2","instruction":indicacion}),
    )
    .unwrap();

    // Una sola llamada al redactor, con la indicación y la misma evidencia
    // verificada del paso 7.
    assert_eq!(reescrita["job"]["status"], "done");
    assert_eq!(reescrita["job"]["llm_calls"], llamadas + 1);
    let pedidos = entradas(&reescrita, "asistente_redaccion");
    assert_eq!(pedidos.len(), 2, "el informe y la reescritura");
    let pedido = &pedidos[1]["data"];
    assert_eq!(pedido["instruction"], indicacion);
    assert_eq!(
        pedido["section"]["text"],
        previo["report"]["sections"][1]["text"]
    );
    assert_eq!(pedido["other_sections"], json!(["Hechos"]));
    assert_eq!(pedido["claims"], pedidos[0]["data"]["claims"]);

    // Versión nueva: cambia solo la sección pedida, y sus citas se arman
    // desde los claims nuevos.
    assert_eq!(informes(&reescrita).len(), 2);
    let nuevo = artefacto(&reescrita, "report");
    let despues = nuevo["report"]["sections"].as_array().unwrap();
    assert_eq!(despues[0], previo["report"]["sections"][0]);
    let s2 = &despues[1];
    assert_eq!(s2["id"], "s2");
    assert_eq!(s2["title"], "Contexto reescrito");
    assert_eq!(s2["origen"], "redactor");
    assert_eq!(s2["version"], 2);
    assert_eq!(s2["indicacion"], indicacion);
    assert_eq!(s2["claim_ids"], json!(["c1"]));
    assert!(!s2["quotes"].as_array().unwrap().is_empty(), "{s2}");
    numeracion_consistente(&nuevo);
    let md = nuevo["markdown"].as_str().unwrap();
    assert!(!md.contains(AVISO_EDICION), "{md}");
    assert_eq!(
        std::fs::read_to_string(dir.join(&id).join("report.md")).unwrap(),
        md
    );
    let evento = eventos(&reescrita, "section_rewritten");
    assert_eq!(evento.len(), 1, "{evento:?}");
    assert_eq!(evento[0]["payload"]["section_id"], "s2");
    assert_eq!(evento[0]["payload"]["version"], 2);
    assert_eq!(evento[0]["payload"]["indicacion"], indicacion);
    assert_eq!(evento[0]["payload"]["fuera_de_presupuesto"], false);

    // Una reescritura que cita claims no verificados se rechaza: el informe
    // queda igual, pero la llamada ya quedó registrada.
    let inventa = redactor_que_cita(json!(["c9"]));
    let error = procesar(
        &db,
        &repo,
        &inventa,
        None,
        &dir,
        json!({"op":"rewrite_section","job_id":id,"section_id":"s1","instruction":"Ampliá"}),
    )
    .unwrap_err();
    assert!(error.contains("no están verificad"), "{error}");
    let despues = procesar(&db, &repo, &m, None, &dir, json!({"op":"get","job_id":id})).unwrap();
    assert_eq!(informes(&despues).len(), 2);
    assert_eq!(artefacto(&despues, "report"), nuevo);
    assert_eq!(despues["job"]["llm_calls"], llamadas + 2);
    assert_eq!(eventos(&despues, "section_rewritten").len(), 1);
}

#[test]
fn con_el_presupuesto_agotado_la_reescritura_igual_corre_y_queda_registrada() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let dir = path.with_extension("presupuesto-seccion-artifacts");
    let m = redactor();

    // Una corrida completa mide cuántas llamadas consume la investigación.
    let medida = EstadoDb::abrir_en_memoria().unwrap();
    let id = create(&medida, &repo, &m, &dir)["job"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let necesarias = correr_con(&medida, &repo, &m, None, &dir, &id)["job"]["llm_calls"]
        .as_i64()
        .unwrap();

    // Con exactamente ese presupuesto, cierra con el presupuesto agotado.
    let state = path.with_extension("presupuesto-seccion-state.sqlite");
    let db = EstadoDb::abrir(state.to_str().unwrap()).unwrap();
    let id = procesar(&db,&repo,&m,None,&dir,json!({"op":"create","question":"¿Hubo huelga?","project":"p","collection_ids":["c-conflicto"],"max_llm_calls":necesarias,"max_cost":1.0})).unwrap()["job"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let cerrada = correr_con(&db, &repo, &m, None, &dir, &id);
    assert_eq!(cerrada["job"]["llm_calls"], cerrada["job"]["max_llm_calls"]);
    let costo = cerrada["job"]["cost"].as_f64().unwrap();

    // El presupuesto frena el gasto automático del agente, no un pedido
    // explícito del historiador.
    let reescrita = procesar(&db,&repo,&m,None,&dir,json!({"op":"rewrite_section","job_id":id,"section_id":"s2","instruction":"Situalo en la huelga de marzo"})).unwrap();
    assert_eq!(reescrita["job"]["llm_calls"], necesarias + 1);
    assert!(
        (reescrita["job"]["cost"].as_f64().unwrap() - costo - 0.01).abs() < 1e-9,
        "{}",
        reescrita["job"]
    );
    assert_eq!(informes(&reescrita).len(), 2);
    let evento = eventos(&reescrita, "section_rewritten");
    assert_eq!(evento[0]["payload"]["fuera_de_presupuesto"], true);

    // La llamada queda asentada como pedida por el historiador; las del
    // agente, no.
    let pedidos = entradas(&reescrita, "asistente_redaccion");
    assert!(pedidos[0].get("pedida_por").is_none(), "{}", pedidos[0]);
    assert_eq!(pedidos[1]["pedida_por"], "historiador");
    let conn = rusqlite::Connection::open(&state).unwrap();
    let (rol, costo_llamada): (String, f64) = conn
        .query_row(
            "SELECT rol,costo FROM llm_calls WHERE job_id=?1 ORDER BY rowid DESC LIMIT 1",
            [&id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(rol, "asistente_redaccion");
    assert!((costo_llamada - 0.01).abs() < 1e-9);
}

#[test]
fn un_informe_sin_ids_admite_editar_por_el_id_asignado_por_orden() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let state = path.with_extension("informe-sin-ids-state.sqlite");
    let dir = path.with_extension("informe-sin-ids-artifacts");
    let m = redactor();
    let db = EstadoDb::abrir(state.to_str().unwrap()).unwrap();
    let id = create(&db, &repo, &m, &dir)["job"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    correr_con(&db, &repo, &m, None, &dir, &id);
    drop(db);

    // Se fuerza un informe anterior al cambio: secciones sin identidad.
    let sin_ids = {
        let conn = rusqlite::Connection::open(&state).unwrap();
        let (artefacto_id, contenido): (String, String) = conn
            .query_row(
                "SELECT id,content_json FROM artifacts WHERE job_id=?1 AND tipo='report'",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let mut contenido: Value = serde_json::from_str(&contenido).unwrap();
        for s in contenido["report"]["sections"].as_array_mut().unwrap() {
            let s = s.as_object_mut().unwrap();
            for campo in ["id", "version", "origen", "indicacion"] {
                s.remove(campo);
            }
        }
        conn.execute(
            "UPDATE artifacts SET content_json=?1 WHERE id=?2",
            rusqlite::params![contenido.to_string(), artefacto_id],
        )
        .unwrap();
        contenido.to_string()
    };
    let db = EstadoDb::abrir(state.to_str().unwrap()).unwrap();

    // Al leerlo, cada sección recibe su id por orden.
    let leido = procesar(&db, &repo, &m, None, &dir, json!({"op":"get","job_id":id})).unwrap();
    let ids: Vec<Value> = secciones(&leido).iter().map(|s| s["id"].clone()).collect();
    assert_eq!(ids, vec![json!("s1"), json!("s2")]);

    let editado = procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"edit_section","job_id":id,"section_id":"s2","text":"Texto del historiador"}),
    )
    .unwrap();
    let nuevas = secciones(&editado);
    assert_eq!(nuevas[0]["id"], "s1");
    assert_eq!(nuevas[0]["version"], 1);
    assert_eq!(nuevas[0]["origen"], "redactor");
    assert_eq!(nuevas[1]["id"], "s2");
    assert_eq!(nuevas[1]["version"], 2);
    assert_eq!(nuevas[1]["origen"], "historiador");
    assert_eq!(nuevas[1]["text"], "Texto del historiador");

    // El informe anterior no se reescribió al leerlo.
    let conn = rusqlite::Connection::open(&state).unwrap();
    let guardado: String = conn
        .query_row(
            "SELECT content_json FROM artifacts WHERE job_id=?1 AND tipo='report' AND version=1",
            [&id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(guardado, sin_ids);
}

#[test]
fn en_una_investigacion_cerrada_las_demas_operaciones_siguen_rechazadas() {
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("cerrada-rechazos-artifacts");
    let m = redactor();
    let id = create(&db, &repo, &m, &dir)["job"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let cerrada = correr_con(&db, &repo, &m, None, &dir, &id);
    // Editar una sección no reabre la investigación.
    procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"edit_section","job_id":id,"section_id":"s1","text":"Texto del historiador"}),
    )
    .unwrap();

    let plan = json!({"queries":["paro"],"bibliography_queries":[],"retrieval_limit":5});
    for pedido in [
        json!({"op":"pause","job_id":id}),
        json!({"op":"resume","job_id":id}),
        json!({"op":"cancel","job_id":id}),
        json!({"op":"update_budget","job_id":id,"max_llm_calls":60,"max_cost":2}),
        json!({"op":"decision","job_id":id,"gate_id":"gate-inexistente","approve":true}),
        json!({"op":"answer","job_id":id,"answers":[{"id":"q1","text":"1965"}]}),
        json!({"op":"revise","job_id":id,"artifact_id":id_vigente(&cerrada, "plan"),"content":plan}),
    ] {
        let error = procesar(&db, &repo, &m, None, &dir, pedido.clone()).unwrap_err();
        assert!(error.contains("cerrada"), "{pedido}: {error}");
    }
    assert!(procesar(
        &db,
        &repo,
        &m,
        None,
        &dir,
        json!({"op":"advance","job_id":id})
    )
    .is_err());

    let despues = procesar(&db, &repo, &m, None, &dir, json!({"op":"get","job_id":id})).unwrap();
    assert_eq!(despues["job"]["status"], "done");
    assert_eq!(despues["job"]["close_reason"], "completed");
    assert_eq!(informes(&despues).len(), 2, "el informe y su edición");
}

#[test]
fn una_seccion_guardada_no_rompe_la_lectura_de_la_investigacion() {
    // `informe_secciones` escribe filas de artefacto en la misma base. Un
    // artefacto sin contenido no puede voltear la lectura del job entero.
    let path = common::crear_corpus_sintetico();
    let repo = RepositorioSqlite::abrir(path.to_str().unwrap()).unwrap();
    let db = EstadoDb::abrir_en_memoria().unwrap();
    let dir = path.with_extension("seccion-suelta-artifacts");
    let m = modelo(false, false);
    let id = create(&db, &repo, &m, &dir)["job"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let secciones = entropia_agent::informe_secciones::InformeSecciones::nuevo(&db, &dir);
    secciones
        .guardar_seccion(
            &id,
            &entropia_agent::informe_secciones::SeccionInforme {
                id: "hechos".into(),
                titulo: "Hechos".into(),
                contenido: "El gremio paró en octubre.".into(),
                version: 1,
                provenance: vec!["chunk-1".into()],
            },
        )
        .expect("la sección se guarda");

    let out = procesar(&db, &repo, &m, None, &dir, json!({"op":"get","job_id":id}))
        .expect("la investigación se sigue leyendo");
    let seccion = out["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == "seccion")
        .expect("la sección figura entre los artefactos");
    assert_eq!(seccion["content"]["provenance"][0], "chunk-1", "{seccion}");
}

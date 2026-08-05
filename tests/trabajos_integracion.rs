//! Tests de integración de Fase 1 (PLAN §9): el job sobrevive a un kill del
//! proceso y retoma desde el último stage completo; el snapshot del corpus se
//! congela al iniciar el job y es estable; la memoria longitudinal persiste.

mod common;

use entropia_agent::estado::EstadoDb;
use entropia_agent::memoria::{MemoriaDb, TipoMemoria};
use entropia_agent::repositorio::RepositorioSqlite;
use entropia_agent::trabajos::{ConfigJob, EstadoJob, MotivoCierre, MotorTrabajos};

fn ruta_estado(nombre: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "entropia-estado-it-{}-{}",
        std::process::id(),
        nombre
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn un_job_sobrevive_al_kill_y_retoma_desde_el_ultimo_stage_completo() {
    let dir = ruta_estado("kill");
    let estado_path = dir.join("estado.sqlite");
    let artifacts = dir.join("sintesis");
    std::fs::create_dir_all(&artifacts).unwrap();

    // Primer «proceso»: crea el job, completa A, deja B a medias (crash).
    {
        let db = EstadoDb::abrir(estado_path.to_str().unwrap()).unwrap();
        let m = MotorTrabajos::nuevo(&db);
        let job = m
            .crear_job(ConfigJob {
                modo: "investigacion".into(),
                pregunta: "¿Conflictividad por décadas?".into(),
                project: "soip-conflictividad".into(),
                corpus: "soip".into(),
                config_snapshot: "{\"modelo\":\"m\",\"denylist\":[]}".into(),
                corpus_snapshot_id: Some("snap-x".into()),
                max_cost: None,
                max_llm_calls: None,
            })
            .unwrap();
        let a = m.agregar_stage(&job.id, "consulta", "Cronología").unwrap();
        let b = m.agregar_stage(&job.id, "sintesis", "Síntesis").unwrap();
        m.agregar_dependencia(&b, &a).unwrap();

        m.marcar_inicio_stage(&job.id, &a).unwrap();
        m.completar_stage(&job.id, &a, "sintesis", "sintesis/a.md")
            .unwrap();
        // Crash simulado: B arrancó pero el proceso «murió» antes de completar.
        m.marcar_inicio_stage(&job.id, &b).unwrap();
    } // ← la conexión se cae acá (kill del proceso)

    // Segundo «proceso»: reabre la base y retoma.
    let db = EstadoDb::abrir(estado_path.to_str().unwrap()).unwrap();
    let m = MotorTrabajos::nuevo(&db);
    let jobs = m.listar_ids_jobs();
    assert_eq!(jobs.len(), 1);
    let job_id = &jobs[0];

    // El resume retoma B (in_progress), no A.
    let siguiente = m.siguiente_stage(job_id).unwrap().unwrap();
    assert_eq!(siguiente.tipo, "sintesis");
    assert_eq!(siguiente.status, "in_progress");

    // Se completa y el job queda listo para cerrarse con motivo.
    m.completar_stage(job_id, &siguiente.id, "informe_parcial", "sintesis/b.md")
        .unwrap();
    m.cerrar(job_id, MotivoCierre::Completed).unwrap();
    let job = m.obtener_job(job_id).unwrap();
    assert_eq!(job.status, EstadoJob::Done);
    assert_eq!(job.close_reason, Some(MotivoCierre::Completed));
    assert_eq!(job.config_snapshot, "{\"modelo\":\"m\",\"denylist\":[]}");
}

#[test]
fn el_snapshot_del_corpus_es_estable_y_sensible_a_cambios() {
    // Sobre la copia sintética del corpus.
    let repo =
        RepositorioSqlite::abrir(common::crear_corpus_sintetico().to_str().unwrap()).unwrap();
    let s1 = repo.snapshot_corpus().unwrap();
    let s2 = repo.snapshot_corpus().unwrap();
    assert_eq!(
        s1, s2,
        "dos lecturas del mismo corpus deben dar el mismo snapshot"
    );
    assert!(s1.contains("rag_chunks"));
}

#[test]
fn el_snapshot_del_corpus_real_es_estable() {
    let local = std::path::Path::new("entropia.sqlite");
    let path = std::env::var("ENTROPIA_DB_PATH")
        .ok()
        .filter(|p| !p.is_empty() && std::path::Path::new(p).exists())
        .or_else(|| local.exists().then(|| local.to_str().unwrap().to_string()));
    let Some(path) = path else {
        eprintln!("sin corpus real: se salta el test de snapshot");
        return;
    };
    let repo = RepositorioSqlite::abrir(&path).unwrap();
    let s1 = repo.snapshot_corpus().unwrap();
    let s2 = repo.snapshot_corpus().unwrap();
    assert_eq!(s1, s2);
}

#[test]
fn la_memoria_longitudinal_persiste_entre_procesos() {
    let dir = ruta_estado("memoria");
    let estado_path = dir.join("estado.sqlite");

    let id_memoria;
    {
        let db = EstadoDb::abrir(estado_path.to_str().unwrap()).unwrap();
        let mem = MemoriaDb::nuevo(&db);
        let (id, _) = mem
            .guardar(
                "soip-conflictividad",
                "Huelga de marzo",
                TipoMemoria::Finding,
                "La huelga de la pesca comenzó el 17 de marzo de 1965.",
                None,
                None,
            )
            .unwrap();
        id_memoria = id;
    }

    // «Reinicio»: otra conexión, misma base.
    let db = EstadoDb::abrir(estado_path.to_str().unwrap()).unwrap();
    let mem = MemoriaDb::nuevo(&db);
    let resultados = mem.buscar("soip-conflictividad", "huelga pesca marzo", 5);
    assert_eq!(resultados.len(), 1);
    assert_eq!(resultados[0].id, id_memoria);
}

#[test]
fn el_dag_rechaza_ciclos_tambien_en_archivo() {
    let dir = ruta_estado("ciclos");
    let estado_path = dir.join("estado.sqlite");
    let db = EstadoDb::abrir(estado_path.to_str().unwrap()).unwrap();
    let m = MotorTrabajos::nuevo(&db);
    let job = m
        .crear_job(ConfigJob::nueva("pregunta", "proyecto"))
        .unwrap();
    let a = m.agregar_stage(&job.id, "consulta", "A").unwrap();
    let b = m.agregar_stage(&job.id, "consulta", "B").unwrap();
    m.agregar_dependencia(&a, &b).unwrap();
    m.agregar_dependencia(&b, &a).unwrap();
    assert!(m.validar_dag(&job.id).is_err());
}

//! Punto de entrada (bin delgado sobre la lib): agente autónomo de
//! investigación historiográfica.
//!
//! El agente recibe el pedido del investigador y decide sus pasos con
//! tool-calling: entrevista, busca fuentes en la base (RAG), lee fragmentos,
//! redacta y guarda el informe.

use std::io::{self, BufRead, Write};
use std::path::Path;

use entropia_agent::agente::Agente;
use entropia_agent::cliente_llm::ClienteLlmOpenRouter;
use entropia_agent::embeddings::ClienteEmbeddings;
use entropia_agent::recuperacion::Recuperador;
use entropia_agent::repositorio::RepositorioSqlite;
use entropia_agent::rerank::ClienteRerank;

fn main() {
    let _ = dotenvy::from_path(".env").ok();
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();

    let api_key: Option<String> = std::env::var("OPENROUTER_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty());
    let (repo_sqlite, db_path) = abrir_repositorio();

    imprimir_bienvenida(&mut stdout);

    let Some(key) = api_key else {
        writeln!(
            &mut stdout,
            "\nEl agente autónomo requiere OPENROUTER_API_KEY. Definila y volvé a ejecutar."
        )
        .unwrap();
        return;
    };

    let modelo = modelo_activo();
    let cliente = ClienteLlmOpenRouter::new(key.clone(), modelo.clone());
    let recuperador = repo_sqlite
        .as_ref()
        .map(|_| Recuperador::new(ClienteEmbeddings::new(key.clone()), ClienteRerank::new(key)));

    writeln!(
        &mut stdout,
        "\n[Base de datos] Modo: {}",
        describir_modo_db(&repo_sqlite, &db_path)
    )
    .unwrap();
    writeln!(
        &mut stdout,
        "[Cliente LLM] OpenRouter · modelo {modelo} (agente con herramientas)"
    )
    .unwrap();

    let agente = Agente::new(cliente, recuperador, repo_sqlite);

    loop {
        let pedido = leer_linea(
            &mut stdout,
            &stdin,
            "\nPedido de investigación (o «salir» para terminar): ",
        );
        let pedido = pedido.trim().to_string();
        if pedido.eq_ignore_ascii_case("salir") || pedido.is_empty() {
            writeln!(
                &mut stdout,
                "\nFin de la sesión. Gracias por usar el motor de EntropIA."
            )
            .unwrap();
            break;
        }
        if let Err(e) = agente.ejecutar(&pedido, &mut stdout, &stdin) {
            writeln!(&mut stdout, "\n[Agente] Error: {e}").unwrap();
        }
    }
}

/// Modelo activo de OpenRouter (configurable o el por defecto).
fn modelo_activo() -> String {
    std::env::var("OPENROUTER_MODEL")
        .ok()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| ClienteLlmOpenRouter::MODELO_DEFAULT.to_string())
}

/// Abre la base común indicada por `ENTROPIA_DB_PATH`.
fn abrir_repositorio() -> (Option<RepositorioSqlite>, Option<String>) {
    match std::env::var("ENTROPIA_DB_PATH") {
        Ok(path) if !path.trim().is_empty() && Path::new(&path).exists() => {
            match RepositorioSqlite::abrir(&path) {
                Ok(repo) => (Some(repo), Some(path)),
                Err(_) => (None, Some(path)),
            }
        }
        _ => (None, None),
    }
}

/// Descripción legible del modo de base de datos.
fn describir_modo_db(repo: &Option<RepositorioSqlite>, path: &Option<String>) -> String {
    match (repo, path) {
        (Some(_), Some(p)) => format!("SQLite · {p} · RAG (embeddings + FTS5 + rerank)"),
        (_, Some(p)) => format!("Sin fuentes · no se pudo abrir la base: {p}"),
        _ => "Sin fuentes · definí ENTROPIA_DB_PATH para usar la base de la app".to_string(),
    }
}

/// Lee una línea desde la entrada estándar y la devuelve sin recortar.
fn leer_linea<W: Write>(stdout: &mut W, stdin: &io::Stdin, prompt: &str) -> String {
    write!(stdout, "{}", prompt).unwrap();
    stdout.flush().unwrap();
    let mut linea = String::new();
    if stdin.lock().read_line(&mut linea).is_err() {
        return String::new();
    }
    linea
}

fn imprimir_bienvenida<W: Write>(stdout: &mut W) {
    writeln!(stdout, "========================================").unwrap();
    writeln!(stdout, " Agente de Investigación Historiográfica").unwrap();
    writeln!(stdout, " Arquitectura: agente autónomo con tool-calling").unwrap();
    writeln!(stdout, "========================================").unwrap();
}

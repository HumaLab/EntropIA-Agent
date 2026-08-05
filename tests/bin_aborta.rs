//! Tests del bin real (Fase 0, PLAN §9): sin base de datos el proceso aborta
//! con código de salida no nulo en lugar de entrar al loop de pedidos.

use std::io::Write;
use std::process::{Command, Stdio};

/// Ejecuta el bin real con el entorno mínimo y devuelve su salida.
fn ejecutar_bin(envs: &[(&str, &str)]) -> std::process::Output {
    let bin = env!("CARGO_BIN_EXE_entropia-agent");
    let mut cmd = Command::new(bin);
    cmd.env_remove("ENTROPIA_DB_PATH")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("ENTROPIA_STATE_PATH")
        .env_remove("OPENROUTER_MODEL");
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("el bin debe poder ejecutarse")
        .wait_with_output()
        .expect("el bin debe terminar")
}

#[test]
fn el_bin_aborta_sin_entropia_db_path_con_codigo_no_nulo() {
    let mut child = {
        let bin = env!("CARGO_BIN_EXE_entropia-agent");
        let mut cmd = Command::new(bin);
        cmd.env_remove("ENTROPIA_DB_PATH")
            .env_remove("OPENROUTER_API_KEY")
            .env_remove("ENTROPIA_STATE_PATH");
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    // Aunque se escriba un pedido, el proceso debe abortar antes de leerlo.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"pedido de prueba\nsalir\n");
    }
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "sin base el bin debe abortar con código no nulo, salió con {}",
        output.status
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stdout.contains("ENTROPIA_DB_PATH"),
        "el error debe nombrar la variable: {stdout}"
    );
    assert!(
        stdout.contains("no produce informes sin fuentes"),
        "el error debe explicar el motivo: {stdout}"
    );
    // No debe entrar al loop de pedidos (ni mostrar el prompt).
    assert!(
        !stdout.contains("Pedido de investigación"),
        "no debe entrar al loop: {stdout}"
    );
}

#[test]
fn el_bin_aborta_si_la_base_no_existe() {
    let output = ejecutar_bin(&[("ENTROPIA_DB_PATH", "ruta/inexistente.sqlite")]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout.contains("no se pudo abrir la base"));
}

#[test]
fn el_bin_con_base_entra_al_loop_y_sale_limpio() {
    let corpus = std::path::Path::new("entropia.sqlite");
    if !corpus.exists() {
        eprintln!("sin corpus real: se salta el test de arranque con base");
        return;
    }
    let mut child = {
        let bin = env!("CARGO_BIN_EXE_entropia-agent");
        let mut cmd = Command::new(bin);
        cmd.env("ENTROPIA_DB_PATH", corpus.to_str().unwrap())
            .env("OPENROUTER_API_KEY", "clave-dummy")
            .env(
                "ENTROPIA_STATE_PATH",
                std::env::temp_dir()
                    .join(format!("entropia-bin-state-{}.sqlite", std::process::id())),
            );
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"salir\n");
    }
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "con base y «salir» el bin debe cerrar limpio: {stdout}"
    );
    assert!(stdout.contains("SQLite"));
    assert!(stdout.contains("Pedido de investigación"));
    assert!(stdout.contains("Fin de la sesión"));
}

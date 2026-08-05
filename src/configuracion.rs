//! Configuración del agente.
//!
//! La denylist de colecciones de prueba es configuración explícita y
//! versionada (PLAN §6.5): las colecciones de stress-test quedan fuera de la
//! recuperación y de los listados, y se congelan en el snapshot de
//! reproducibilidad de cada job (§6.9). No es una heurística sobre el nombre:
//! es una lista explícita, y entra en el `config_snapshot` de cada job.

/// Colecciones de prueba/stress excluidas de la recuperación y de los listados.
///
/// Verificadas sobre el corpus real (2026-08-05): `stress222` y
/// `stress-test-01` son las dos colecciones «más grandes» que ve el agente y
/// aportan 266 de 1.648 chunks (16 % de ruido).
pub const COLECCIONES_EXCLUIDAS: [&str; 6] = [
    "stress222",
    "stress-test-01",
    "validacioneventos2222",
    "nuevo proyecto",
    "probar2",
    "prueba3",
];

/// Devuelve la lista de colecciones excluidas (para SQL `NOT IN`).
pub fn colecciones_excluidas() -> Vec<String> {
    COLECCIONES_EXCLUIDAS
        .iter()
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_denylist_tiene_las_seis_colecciones_de_prueba() {
        let excluidas = colecciones_excluidas();
        assert_eq!(excluidas.len(), 6);
        for nombre in COLECCIONES_EXCLUIDAS {
            assert!(excluidas.contains(&nombre.to_string()));
        }
    }
}

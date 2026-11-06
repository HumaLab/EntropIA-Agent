//! Parámetros del agente: defaults que reproducen lo hardcodeado, validación
//! con mensajes que nombran el campo y su rango, y el contrato JSON que el
//! desktop manda en `parameters`.

use entropia_agent::parametros::ParametrosAgente;
use serde_json::{json, Value};

/// Error de un `parameters` que tiene que rechazarse.
fn error_de(valor: Value) -> String {
    match ParametrosAgente::desde_valor(&valor) {
        Ok(p) => panic!("se esperaba un rechazo y los parámetros válidos son {p:?}"),
        Err(e) => e,
    }
}

#[test]
fn los_defaults_reproducen_los_valores_hardcodeados_de_hoy() {
    let p = ParametrosAgente::default();
    assert_eq!(p.max_tokens_salida, 16_384, "tope de salida del cliente");
    assert_eq!(p.temperatura, 0.3, "temperatura del pedido al modelo");
    assert_eq!(p.timeout_s, 180, "timeout total del cliente");
    assert_eq!(
        p.esperas_reintento_s,
        vec![2, 6],
        "esperas entre reintentos"
    );
    assert_eq!(
        p.lotes_archivo_bytes,
        vec![48_000, 24_000, 12_000],
        "escalera de lotes de archivo"
    );
    assert_eq!(p.fragmento_archivo_bytes, 8_000, "fragmento de evidencia");
    assert_eq!(p.ventana_cita, 600, "ventana de la cita en el informe");
    assert_eq!(p.preguntas_minimas, 4, "piso de preguntas de la ronda");
    assert_eq!(p.rerank_depth, 16, "candidatos que entran al rerank");
    assert_eq!(p.leg_k, 24, "candidatos por pierna de recuperación");
    assert_eq!(p.rrf_k, 60, "suavizado de RRF");
    assert_eq!(p.snippet_max, 1_600, "tope de caracteres por fragmento");
    assert_eq!(p.max_consultas.get("general"), Some(&20));
    assert_eq!(p.max_consultas.get("trayectorias"), Some(&30));
    assert_eq!(p.max_consultas.get("cronologia"), Some(&20));
    assert_eq!(p.max_consultas.len(), 3, "una cuota por modalidad");
}

#[test]
fn un_valor_fuera_de_rango_se_rechaza_nombrando_el_campo_y_su_rango() {
    assert_eq!(
        error_de(json!({"max_tokens_salida": 10})),
        "max_tokens_salida debe estar entre 1024 y 131072 (recibido: 10)"
    );
    assert_eq!(
        error_de(json!({"temperatura": 2.5})),
        "temperatura debe estar entre 0.0 y 2.0 (recibido: 2.5)"
    );
    assert_eq!(
        error_de(json!({"timeout_s": 0})),
        "timeout_s debe estar entre 1 y 3600 (recibido: 0)"
    );
    assert_eq!(
        error_de(json!({"esperas_reintento_s": [0, 0, 0, 0, 0, 0, 0, 0, 0]})),
        "esperas_reintento_s debe tener entre 0 y 8 valores (recibido: 9)"
    );
    assert_eq!(
        error_de(json!({"lotes_archivo_bytes": [48_000, 24_000, 12_000, 6_000, 3_000, 1_500]})),
        "lotes_archivo_bytes debe tener entre 1 y 5 valores (recibido: 6)"
    );
    assert_eq!(
        error_de(json!({"lotes_archivo_bytes": [500]})),
        "lotes_archivo_bytes[0] debe estar entre 1000 y 1000000 (recibido: 500)"
    );
}

#[test]
fn una_escalera_que_no_decrece_se_rechaza() {
    assert_eq!(
        error_de(json!({"lotes_archivo_bytes": [48_000, 48_000]})),
        "lotes_archivo_bytes debe ser estrictamente decreciente: lotes_archivo_bytes[1] (48000) no es menor que lotes_archivo_bytes[0] (48000)"
    );
    assert_eq!(
        error_de(json!({"lotes_archivo_bytes": [24_000, 48_000]})),
        "lotes_archivo_bytes debe ser estrictamente decreciente: lotes_archivo_bytes[1] (48000) no es menor que lotes_archivo_bytes[0] (24000)"
    );
}

#[test]
fn un_fragmento_mas_grande_que_el_primer_lote_se_rechaza() {
    assert_eq!(
        error_de(json!({"lotes_archivo_bytes": [4_000], "fragmento_archivo_bytes": 8_000})),
        "fragmento_archivo_bytes (8000) no puede superar el primer lote de archivo (4000)"
    );
}

#[test]
fn un_perfil_desconocido_en_max_consultas_se_rechaza() {
    assert_eq!(
        error_de(json!({"max_consultas": {"biografia": 10}})),
        "max_consultas nombra un perfil desconocido: «biografia»; disponibles: general, trayectorias, cronologia"
    );
}

#[test]
fn un_campo_desconocido_se_rechaza_nombrando_los_validos() {
    assert_eq!(
        error_de(json!({"temperaturaa": 0.5})),
        "Parámetros del agente: campo desconocido «temperaturaa». Campos válidos: max_tokens_salida, temperatura, timeout_s, esperas_reintento_s, lotes_archivo_bytes, fragmento_archivo_bytes, ventana_cita, preguntas_minimas, rerank_depth, leg_k, rrf_k, snippet_max, max_consultas"
    );
}

#[test]
fn un_objeto_parcial_se_completa_con_los_defaults_y_se_serializa_entero() {
    let p = ParametrosAgente::desde_valor(&json!({"preguntas_minimas": 6})).unwrap();
    assert_eq!(p.preguntas_minimas, 6);
    assert_eq!(p.rerank_depth, 16, "lo que no vino queda en su default");
    // El artefacto congelado guarda la estructura completa: el desktop lee de
    // ahí lo que el job usa de verdad.
    let ida = serde_json::to_value(&p).unwrap();
    assert_eq!(ida["preguntas_minimas"], json!(6));
    assert_eq!(
        ida["max_consultas"],
        json!({"cronologia": 20, "general": 20, "trayectorias": 30})
    );
    assert_eq!(ParametrosAgente::desde_valor(&ida).unwrap(), p);
}

#[test]
fn sin_parameters_rigen_los_defaults() {
    assert_eq!(
        ParametrosAgente::desde_valor(&Value::Null).unwrap(),
        ParametrosAgente::default()
    );
    assert_eq!(
        ParametrosAgente::desde_valor(&json!({})).unwrap(),
        ParametrosAgente::default()
    );
}

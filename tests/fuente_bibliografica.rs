//! Contrato de la fuente bibliográfica: ubicaciones, etiquetas de cita y
//! serialización de los pasajes que entran al workflow como evidencia.
use entropia_agent::fuente_bibliografica::{
    FuenteBibliografica, PasajeBibliografico, TipoUbicacion, Ubicacion,
};
use serde_json::json;

#[test]
fn la_etiqueta_de_ubicacion_distingue_paginas_y_parrafos() {
    assert_eq!(
        Ubicacion {
            tipo: TipoUbicacion::Paginas,
            desde: 3,
            hasta: 3
        }
        .etiqueta(),
        "p. 3"
    );
    assert_eq!(
        Ubicacion {
            tipo: TipoUbicacion::Paginas,
            desde: 3,
            hasta: 4
        }
        .etiqueta(),
        "pp. 3–4"
    );
    assert_eq!(
        Ubicacion {
            tipo: TipoUbicacion::Parrafos,
            desde: 2,
            hasta: 2
        }
        .etiqueta(),
        "párr. 2"
    );
    assert_eq!(
        Ubicacion {
            tipo: TipoUbicacion::Parrafos,
            desde: 2,
            hasta: 5
        }
        .etiqueta(),
        "párr. 2–5"
    );
}

#[test]
fn el_localizador_del_ledger_es_el_rango_del_libro() {
    assert_eq!(
        Ubicacion {
            tipo: TipoUbicacion::Paginas,
            desde: 3,
            hasta: 3
        }
        .localizador(),
        "3"
    );
    assert_eq!(
        Ubicacion {
            tipo: TipoUbicacion::Paginas,
            desde: 3,
            hasta: 4
        }
        .localizador(),
        "3-4"
    );
    assert_eq!(
        Ubicacion {
            tipo: TipoUbicacion::Parrafos,
            desde: 2,
            hasta: 2
        }
        .localizador(),
        "2"
    );
}

#[test]
fn la_ubicacion_viaja_como_json_en_snake_case() {
    let u = Ubicacion {
        tipo: TipoUbicacion::Paginas,
        desde: 3,
        hasta: 4,
    };
    let v = serde_json::to_value(u).unwrap();
    assert_eq!(v, json!({"tipo":"paginas","desde":3,"hasta":4}));
    let vuelta: Ubicacion = serde_json::from_value(v).unwrap();
    assert_eq!(vuelta.desde, 3);
    assert_eq!(vuelta.hasta, 4);
    assert_eq!(
        serde_json::to_value(TipoUbicacion::Parrafos).unwrap(),
        json!("parrafos")
    );
}

#[test]
fn el_pasaje_serializa_sus_campos() {
    let p = PasajeBibliografico {
        chunk_id: "cap-3".into(),
        item_key: "LIBRO-A".into(),
        titulo: "Historia de la huelga general".into(),
        autores: "Ana González".into(),
        anio: Some(1965),
        biblioteca: "user:123".into(),
        texto: "La huelga general de 1965 quedó registrada en las actas.".into(),
        ubicacion: Some(Ubicacion {
            tipo: TipoUbicacion::Paginas,
            desde: 3,
            hasta: 4,
        }),
    };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["chunk_id"], "cap-3");
    assert_eq!(v["item_key"], "LIBRO-A");
    assert_eq!(v["biblioteca"], "user:123");
    assert_eq!(v["anio"], 1965);
    assert_eq!(v["ubicacion"]["tipo"], "paginas");
    // Sin ubicación el pasaje sigue siendo un pasaje: el campo viaja nulo.
    let sin = PasajeBibliografico {
        ubicacion: None,
        ..p
    };
    assert!(serde_json::to_value(&sin).unwrap()["ubicacion"].is_null());
}

/// Un pasaje con ubicación respeta el límite que pide el plan.
struct FuenteAcotada;
impl FuenteBibliografica for FuenteAcotada {
    fn buscar(&self, _consulta: &str, limite: usize) -> Result<Vec<PasajeBibliografico>, String> {
        Ok((0..limite)
            .map(|i| PasajeBibliografico {
                chunk_id: format!("p{i}"),
                item_key: "LIBRO".into(),
                titulo: "Libro".into(),
                autores: String::new(),
                anio: None,
                biblioteca: "user:123".into(),
                texto: "texto".into(),
                ubicacion: None,
            })
            .collect())
    }
}

#[test]
fn el_trait_es_objeto_dinamico_enviable() {
    let fuente: &dyn FuenteBibliografica = &FuenteAcotada;
    let pasajes = fuente.buscar("x", 1).unwrap();
    assert_eq!(pasajes.len(), 1);
}

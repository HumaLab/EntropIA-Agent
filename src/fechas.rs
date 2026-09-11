//! Reglas de fechas del corpus SOIP (PLAN §6.5, Fase 2).
//!
//! **`document_date`** — la fecha del documento, con candidatos por capas según
//! la colección (verificado 2026-08-05):
//!
//! | Formato | Ejemplo | Colección |
//! |---|---|---|
//! | `YY-MM-DD[-sufijo]` | `65-03-17-a` | Conflicto SOIP 1965-66 |
//! | `YYYY-MM-DD - texto` | `1964-12-23 - AOMA` | AOMA, Volantes |
//! | `DD-MM-YYYY` | `B - 23-07-2011` | Voces |
//! | Parcial con comodín | `1965-00-0x - AOMA` | AOMA |
//! | Sin fecha | `IMG_2991` | SOIP 1961 |
//! | Numérico que **no** es fecha | `54`, `151` (nº de resolución) | Resoluciones SOIP |
//!
//! Capas de respaldo: año de la colección, carpeta original (`originalPath`) y
//! dateline en el texto (formas españolas). Cada candidato lleva precision +
//! confidence + source.
//!
//! **Fechas mencionadas** — fechas de *eventos* dentro del documento (timeline):
//! son claims factuales con dimensión temporal, no evidence.

/// Precisión de un candidato de fecha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Precision {
    Ninguna,
    Anio,
    Mes,
    Dia,
}

impl Precision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Precision::Ninguna => "none",
            Precision::Anio => "year",
            Precision::Mes => "month",
            Precision::Dia => "day",
        }
    }
}

/// Una fecha del documento.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fecha {
    pub anio: i64,
    pub mes: Option<i64>,
    pub dia: Option<i64>,
}

impl Fecha {
    /// Representación ISO `YYYY-MM-DD` (con `00` en los campos ausentes).
    pub fn iso(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}",
            self.anio,
            self.mes.unwrap_or(0),
            self.dia.unwrap_or(0)
        )
    }
}

/// Un candidato de `document_date` con su origen.
#[derive(Debug, Clone)]
pub struct CandidatoFecha {
    pub fecha: Option<Fecha>,
    pub precision: Precision,
    pub confidence: f64,
    pub source: String,
}

/// Una fecha mencionada (timeline) dentro del texto de un documento.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FechaMencionada {
    pub texto: String,
    pub fecha: Fecha,
}

const MESES: [(&str, i64); 13] = [
    ("enero", 1),
    ("febrero", 2),
    ("marzo", 3),
    ("abril", 4),
    ("mayo", 5),
    ("junio", 6),
    ("julio", 7),
    ("agosto", 8),
    ("septiembre", 9),
    ("setiembre", 9),
    ("octubre", 10),
    ("noviembre", 11),
    ("diciembre", 12),
];

/// Convierte `YY` a año completo (corpus 1930–1970 → siglo XX; `17` → 2017
/// solo si el año es 2000+ por el formato, se asume 19XX por el corpus).
fn anio_corto(yy: i64) -> i64 {
    1900 + yy
}

/// `document_date` desde el título, según las capas de la tabla de §6.5.
pub fn fecha_desde_titulo(titulo: &str, coleccion: &str) -> Option<CandidatoFecha> {
    let t = titulo.trim();

    // Capa 1: YY-MM-DD[-sufijo] (Conflicto SOIP 1965-66: «65-03-17-a»).
    if let Some(f) = parse_yy_mm_dd(t) {
        return Some(CandidatoFecha {
            fecha: Some(f),
            precision: Precision::Dia,
            confidence: 0.95,
            source: "titulo:yy-mm-dd".into(),
        });
    }

    // Capa 2: YYYY-MM-DD - texto (AOMA, Volantes: «1964-12-23 - AOMA»).
    if let Some(f) = parse_yyyy_mm_dd(t) {
        return Some(CandidatoFecha {
            fecha: Some(f),
            precision: Precision::Dia,
            confidence: 0.95,
            source: "titulo:yyyy-mm-dd".into(),
        });
    }

    // Capa 3: DD-MM-YYYY (Voces: «B - 23-07-2011»).
    if let Some(f) = parse_dd_mm_yyyy(t) {
        return Some(CandidatoFecha {
            fecha: Some(f),
            precision: Precision::Dia,
            confidence: 0.9,
            source: "titulo:dd-mm-yyyy".into(),
        });
    }

    // Capa 4: parcial con comodín (AOMA: «1965-00-0x - AOMA»).
    if let Some(f) = parse_parcial(t) {
        return Some(CandidatoFecha {
            fecha: Some(f),
            precision: Precision::Anio,
            confidence: 0.6,
            source: "titulo:parcial".into(),
        });
    }

    // Capa 5: año suelto en el título (p. ej. «SOIP 1961» dentro de un item).
    if let Some(f) = parse_anio_suelto(t) {
        return Some(CandidatoFecha {
            fecha: Some(f),
            precision: Precision::Anio,
            confidence: 0.5,
            source: "titulo:anio".into(),
        });
    }

    // Capa 6: numérico que no es fecha (resoluciones: «54», «151»).
    if es_numero_puro(t) {
        return Some(CandidatoFecha {
            fecha: None,
            precision: Precision::Ninguna,
            confidence: 0.8,
            source: "titulo:numerico-no-fecha".into(),
        });
    }

    // Capa de respaldo: año de la colección («SOIP 1961»).
    if let Some(anio) = anio_de_coleccion(coleccion) {
        return Some(CandidatoFecha {
            fecha: Some(Fecha {
                anio,
                mes: None,
                dia: None,
            }),
            precision: Precision::Anio,
            confidence: 0.4,
            source: "coleccion".into(),
        });
    }

    None
}

/// Capa de respaldo: año en la carpeta original (`originalPath`, «LC - Huelga
/// SOIP julio 1961»).
pub fn anio_de_original_path(original_path: &str) -> Option<i64> {
    parse_anio_suelto(original_path).map(|f| f.anio)
}

/// `document_date` por capas completas (PLAN §6.5): título → colección →
/// carpeta original. El texto del asset es la última capa y la anota el worker
/// al leer (no es determinista por sí sola).
pub fn document_date(
    titulo: &str,
    coleccion: &str,
    original_path: Option<&str>,
) -> Option<CandidatoFecha> {
    if let Some(c) = fecha_desde_titulo(titulo, coleccion) {
        return Some(c);
    }
    if let Some(path) = original_path {
        if let Some(anio) = anio_de_original_path(path) {
            return Some(CandidatoFecha {
                fecha: Some(Fecha {
                    anio,
                    mes: None,
                    dia: None,
                }),
                precision: Precision::Anio,
                confidence: 0.35,
                source: "original_path".into(),
            });
        }
    }
    None
}

/// Año de la colección: «SOIP 1961» → 1961; «Conflicto SOIP 1965-66» → 1965.
pub fn anio_de_coleccion(coleccion: &str) -> Option<i64> {
    let tokens: Vec<&str> = coleccion.split(|c: char| !c.is_ascii_digit()).collect();
    for t in tokens {
        if t.len() == 4 {
            if let Ok(anio) = t.parse::<i64>() {
                if (1900..=2100).contains(&anio) {
                    return Some(anio);
                }
            }
        }
    }
    None
}

/// Fechas mencionadas (timeline) en el texto: formas españolas deterministas.
/// Tras un acierto se salta el texto consumido (evita dobles lecturas como
/// «17 de marzo» y «7 de marzo» en la misma posición).
pub fn fechas_mencionadas(texto: &str) -> Vec<FechaMencionada> {
    let mut resultado = Vec::new();
    let mut saltar_hasta: usize = 0;
    for (i, _c) in texto.char_indices() {
        if i < saltar_hasta {
            continue;
        }
        let resto = &texto[i..];
        // «17 de marzo de 1965»
        if let Some((f, largo)) = parse_mes_literal(resto) {
            resultado.push(FechaMencionada {
                texto: resto[..largo].to_string(),
                fecha: f,
            });
            saltar_hasta = i + largo;
            continue;
        }
        // «17/03/65» y «17-03-1965»
        if let Some((f, largo)) = parse_numerica(resto) {
            resultado.push(FechaMencionada {
                texto: resto[..largo].to_string(),
                fecha: f,
            });
            saltar_hasta = i + largo;
            continue;
        }
    }
    dedup_consecutivas(resultado)
}

fn dedup_consecutivas(v: Vec<FechaMencionada>) -> Vec<FechaMencionada> {
    let mut out: Vec<FechaMencionada> = Vec::new();
    for item in v {
        if out.last().map(|l| l.fecha == item.fecha).unwrap_or(false) {
            continue;
        }
        out.push(item);
    }
    out
}

// ── parsers ────────────────────────────────────────────────────────────────

fn parse_yy_mm_dd(t: &str) -> Option<Fecha> {
    let partes = t.get(..8)?;
    let mut it = partes.split('-');
    let (yy, mm, dd) = (it.next()?, it.next()?, it.next()?);
    let (yy, mm, dd) = (
        yy.parse::<i64>().ok()?,
        mm.parse::<i64>().ok()?,
        dd.parse::<i64>().ok()?,
    );
    if (0..=99).contains(&yy) && (1..=12).contains(&mm) && (1..=31).contains(&dd) {
        Some(Fecha {
            anio: anio_corto(yy),
            mes: Some(mm),
            dia: Some(dd),
        })
    } else {
        None
    }
}

fn parse_yyyy_mm_dd(t: &str) -> Option<Fecha> {
    let partes = t.get(..10)?;
    let mut it = partes.split('-');
    let (yy, mm, dd) = (it.next()?, it.next()?, it.next()?);
    let (yy, mm, dd) = (
        yy.parse::<i64>().ok()?,
        mm.parse::<i64>().ok()?,
        dd.parse::<i64>().ok()?,
    );
    if (1900..=2100).contains(&yy) && (1..=12).contains(&mm) && (1..=31).contains(&dd) {
        Some(Fecha {
            anio: yy,
            mes: Some(mm),
            dia: Some(dd),
        })
    } else {
        None
    }
}

fn parse_dd_mm_yyyy(t: &str) -> Option<Fecha> {
    // Busca DD-MM-YYYY en cualquier posición (p. ej. «B - 23-07-2011»).
    for (i, _) in t.char_indices() {
        let Some(ventana) = t.get(i..i + 10) else {
            continue;
        };
        let mut it = ventana.split('-');
        let (Some(dd_s), Some(mm_s), Some(yy_s)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let (Ok(dd), Ok(mm), Ok(yy)) = (
            dd_s.parse::<i64>(),
            mm_s.parse::<i64>(),
            yy_s.parse::<i64>(),
        ) else {
            continue;
        };
        if (1..=31).contains(&dd) && (1..=12).contains(&mm) && (1900..=2100).contains(&yy) {
            return Some(Fecha {
                anio: yy,
                mes: Some(mm),
                dia: Some(dd),
            });
        }
    }
    None
}

fn parse_parcial(t: &str) -> Option<Fecha> {
    // «1965-00-0x»: año con mes/día en cero o comodín.
    let partes = t.get(..10)?;
    let mut it = partes.split('-');
    let (yy, mm, dd) = (it.next()?, it.next()?, it.next()?);
    let yy = yy.parse::<i64>().ok()?;
    let mm_ok = mm.chars().all(|c| c == '0' || c == 'x');
    let dd_ok = dd.chars().all(|c| c == '0' || c == 'x');
    if (1900..=2100).contains(&yy) && mm_ok && dd_ok {
        Some(Fecha {
            anio: yy,
            mes: None,
            dia: None,
        })
    } else {
        None
    }
}

fn parse_anio_suelto(t: &str) -> Option<Fecha> {
    for (i, _) in t.char_indices() {
        let resto = &t[i..];
        let Some(cuatro) = resto.get(..4) else {
            continue;
        };
        if cuatro.chars().all(|c| c.is_ascii_digit()) {
            let anio = cuatro.parse::<i64>().ok()?;
            if (1900..=2100).contains(&anio) {
                // No debe ser parte de una fecha más completa ya cubierta.
                let antes = t[..i].chars().last();
                let despues = resto[4..].chars().next();
                let flanco_ok = antes.map(|c| !c.is_ascii_digit()).unwrap_or(true)
                    && despues
                        .map(|c| !c.is_ascii_digit() && c != '-')
                        .unwrap_or(true);
                if flanco_ok {
                    return Some(Fecha {
                        anio,
                        mes: None,
                        dia: None,
                    });
                }
            }
        }
    }
    None
}

fn es_numero_puro(t: &str) -> bool {
    !t.is_empty() && t.chars().all(|c| c.is_ascii_digit())
}

fn parse_mes_literal(resto: &str) -> Option<(Fecha, usize)> {
    // «(\d{1,2}) de (mes) de (\d{4})»
    let digitos: String = resto.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digitos.is_empty() {
        return None;
    }
    let dia: i64 = digitos.parse().ok()?;
    let despues = &resto[digitos.len()..];
    if !despues.starts_with(" de ") {
        return None;
    }
    let resto2 = &despues[4..];
    for (nombre, mes) in MESES {
        if let Some(resto3) = resto2.strip_prefix(nombre) {
            if let Some(resto4) = resto3.strip_prefix(" de ") {
                let anio_s: String = resto4.chars().take_while(|c| c.is_ascii_digit()).collect();
                if anio_s.len() == 4 {
                    let anio: i64 = anio_s.parse().ok()?;
                    let largo = digitos.len() + 4 + nombre.len() + 4 + 4;
                    return Some((
                        Fecha {
                            anio,
                            mes: Some(mes),
                            dia: Some(dia),
                        },
                        largo,
                    ));
                }
            }
        }
    }
    None
}

fn parse_numerica(resto: &str) -> Option<(Fecha, usize)> {
    // «dd/mm/yyyy» o «dd-mm-yyyy» (o yy)
    let b = resto.as_bytes();
    if b.len() < 8 {
        return None;
    }
    for sep in ['/', '-'] {
        let pos = match resto[..].find(sep) {
            Some(p) if p == 2 => p,
            _ => continue,
        };
        let dd = match resto[..pos].parse::<i64>() {
            Ok(d) => d,
            Err(_) => continue,
        };
        let resto2 = &resto[pos + 1..];
        let pos2 = match resto2.find(sep) {
            Some(p) if p == 2 => p,
            _ => continue,
        };
        let mm = match resto2[..pos2].parse::<i64>() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let anio_s: String = resto2[pos2 + 1..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if anio_s.is_empty() {
            continue;
        }
        let anio = match anio_s.len() {
            4 => match anio_s.parse::<i64>() {
                Ok(a) => a,
                Err(_) => continue,
            },
            2 => anio_corto(match anio_s.parse::<i64>() {
                Ok(a) => a,
                Err(_) => continue,
            }),
            _ => continue,
        };
        if (1..=31).contains(&dd) && (1..=12).contains(&mm) {
            let largo = pos + 1 + pos2 + 1 + anio_s.len();
            return Some((
                Fecha {
                    anio,
                    mes: Some(mm),
                    dia: Some(dd),
                },
                largo,
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titulo_reportado_con_represion_conserva_el_anio() {
        let c = document_date(
            "1948-01-00 - FORA - Represión anti-obrera en MDP enero 1948",
            "",
            None,
        )
        .unwrap();
        assert_eq!(c.fecha.unwrap().iso(), "1948-00-00");
        assert_eq!(c.precision, Precision::Anio);
    }

    #[test]
    fn titulos_unicode_respetan_los_limites_de_las_ventanas() {
        for caracter in ['ó', '中', '𐍈'] {
            for padding in 0..10 {
                let prefijo = format!("{}{caracter}", "a".repeat(padding));
                assert!(fecha_desde_titulo(&prefijo, "").is_none());

                let titulo = format!("{prefijo} - 23-07-2011");
                let c = fecha_desde_titulo(&titulo, "").unwrap();
                assert_eq!(c.fecha.unwrap().iso(), "2011-07-23");
                assert_eq!(c.precision, Precision::Dia);

                let titulo = format!("{prefijo} - enero 1948");
                let c = fecha_desde_titulo(&titulo, "").unwrap();
                assert_eq!(c.fecha.unwrap().iso(), "1948-00-00");
                assert_eq!(c.precision, Precision::Anio);
            }
        }
    }

    #[test]
    fn original_path_unicode_conserva_el_anio_de_respaldo() {
        let c = document_date(
            "Documento suelto",
            "",
            Some("Archivo/Represión obrera/enero 1948"),
        )
        .unwrap();
        assert_eq!(c.fecha.unwrap().iso(), "1948-00-00");
        assert_eq!(c.precision, Precision::Anio);
        assert_eq!(c.source, "original_path");
    }

    #[test]
    fn yy_mm_dd_con_sufijo() {
        let c = fecha_desde_titulo("65-03-17-a", "Conflicto SOIP 1965-66").unwrap();
        assert_eq!(c.precision, Precision::Dia);
        assert_eq!(c.fecha.unwrap().iso(), "1965-03-17");
    }

    #[test]
    fn yyyy_mm_dd_con_texto() {
        let c = fecha_desde_titulo("1964-12-23 - AOMA", "AOMA").unwrap();
        assert_eq!(c.fecha.unwrap().iso(), "1964-12-23");
        assert_eq!(c.precision, Precision::Dia);
    }

    #[test]
    fn dd_mm_yyyy_en_voces() {
        let c = fecha_desde_titulo("B - 23-07-2011", "Voces").unwrap();
        assert_eq!(c.fecha.unwrap().iso(), "2011-07-23");
    }

    #[test]
    fn parcial_con_comodin() {
        let c = fecha_desde_titulo("1965-00-0x - AOMA", "AOMA").unwrap();
        assert_eq!(c.precision, Precision::Anio);
        assert_eq!(c.fecha.unwrap().anio, 1965);
    }

    #[test]
    fn numerico_que_no_es_fecha() {
        let c = fecha_desde_titulo("54", "Resoluciones SOIP").unwrap();
        assert_eq!(c.precision, Precision::Ninguna);
        assert!(c.fecha.is_none());
    }

    #[test]
    fn anio_de_la_coleccion_como_respaldo() {
        let c = fecha_desde_titulo("IMG_2991", "SOIP 1961").unwrap();
        assert_eq!(c.precision, Precision::Anio);
        assert_eq!(c.fecha.unwrap().anio, 1961);
        assert_eq!(c.source, "coleccion");
    }

    #[test]
    fn anio_desde_original_path() {
        assert_eq!(
            anio_de_original_path("LC - Huelga SOIP julio 1961"),
            Some(1961)
        );
    }

    #[test]
    fn fechas_mencionadas_literal() {
        let f = fechas_mencionadas("La huelga comenzó el 17 de marzo de 1965 y duró semanas.");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].fecha.iso(), "1965-03-17");
    }

    #[test]
    fn fechas_mencionadas_numerica() {
        let f = fechas_mencionadas("Acta del 23/07/1965 y del 17-03-65.");
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].fecha.iso(), "1965-07-23");
        assert_eq!(f[1].fecha.iso(), "1965-03-17");
    }

    #[test]
    fn fecha_sin_ninguna_pista() {
        assert!(fecha_desde_titulo("Documento suelto", "Voces").is_none());
    }
}

//! Perfiles de informe: la modalidad que el investigador le pide al agente.
//!
//! Un perfil es **dato, nunca un agente**. No forkea el pipeline: inyecta
//! fragmentos de contrato en cuatro puntos del workflow —clarificación, plan,
//! archivo y redacción— y en ninguno más.
//!
//! Invariante deliberada (§frontera epistémica): la prospección, la
//! verificación y el ensamblado de citas **no reciben el perfil**. El
//! validador juzga entailment contra la evidencia, aislado del productor y sin
//! conocimiento externo; si supiera que valida una biografía empezaría a
//! completar huecos con narrativa plausible. Esa ceguera es la garantía, no un
//! olvido.
//!
//! Contrapartida honesta: un perfil sesga la recuperación y la extracción. Un
//! perfil de cronología que busca fechas encuentra fechas y sub-reporta el
//! resto, y el informe igual queda impecable. Por eso cada perfil declara su
//! sesgo y el render lo imprime junto a la tabla de cobertura: se declara el
//! instrumento como se declara el recorte.

/// Modalidad de informe. Todos los campos son fragmentos de contrato que se
/// concatenan a los contratos base de cada rol.
#[derive(Clone, Copy, Debug)]
pub struct Perfil {
    /// Identificador estable que viaja en el job (`modalidad`).
    pub id: &'static str,
    /// Nombre legible para la cabecera del informe.
    pub nombre: &'static str,
    /// Ejes de las preguntas de clarificación. Fijan el piso de cuatro
    /// preguntas cuando el modelo devuelve menos.
    pub ejes_pregunta: &'static [&'static str],
    /// Cómo expandir las consultas del plan sobre el corpus.
    pub hint_consultas: &'static str,
    /// Qué forma tiene un claim bajo esta modalidad.
    pub forma_claim: &'static str,
    /// Instrucción de ordenamiento para el redactor. No es un re-sort
    /// determinista: el código no puede extraer actor ni fecha del texto sin
    /// inventar, así que la instrucción viaja al modelo y el orden queda a la
    /// vista del investigador.
    pub orden_informe: &'static str,
    /// Sesgo que este perfil introduce. Se imprime en el informe.
    pub sesgo_declarado: &'static str,
    /// Tope de consultas de un plan bajo esta modalidad. Cada consulta es una
    /// búsqueda en el corpus (embeddings y rerank), así que el tope acota
    /// también esas llamadas.
    pub max_consultas: usize,
}

/// Perfil aplicado cuando el job no declara modalidad.
pub const ID_POR_DEFECTO: &str = "general";

const GENERAL: Perfil = Perfil {
    id: "general",
    nombre: "Informe historiográfico general",
    ejes_pregunta: &[
        "Período: ¿qué recorte temporal delimita el informe, y por qué ese y no otro?",
        "Enfoque: ¿qué dimensión del problema interesa (conflicto, organización, discurso, condiciones materiales), y cuál queda fuera?",
        "Fuentes: ¿qué tipos documentales del recorte deben pesar más, y cuáles son marginales?",
        "Criterio de cierre: ¿qué tiene que estar respondido para considerar terminado el informe?",
    ],
    hint_consultas:
        "Cubrí el problema con términos distintos, incluidas variantes de época y del vocabulario de las fuentes.",
    forma_claim: "Cada claim es un hecho documentado con su evidencia.",
    orden_informe: "Ordená las secciones por el hilo argumental del diseño.",
    sesgo_declarado:
        "Sin priorización temática: la recuperación sigue las consultas del plan, que pueden no agotar el corpus.",
    max_consultas: 20,
};

const TRAYECTORIAS: Perfil = Perfil {
    id: "trayectorias",
    nombre: "Reconstrucción de trayectorias de personas y organizaciones",
    ejes_pregunta: &[
        "Actores: ¿qué personas u organizaciones hay que reconstruir? Nombralas de forma explícita.",
        "Variantes del nombre: ¿con qué formas aparecen en las fuentes (iniciales, apodos, cargos usados como referencia, razón social anterior)?",
        "Tramo: ¿qué segmento de la trayectoria interesa, y desde qué momento se la considera relevante?",
        "Cierre de trayectoria: ¿qué cuenta como final del recorrido (muerte, disolución, salida de la organización, fin del período), y qué se hace si la documentación se corta antes?",
    ],
    hint_consultas:
        "Generá consultas por cada variante del nombre de cada actor, por sus cargos y por las organizaciones donde participó: una trayectoria se pierde cuando la fuente nombra al actor de otra manera.",
    forma_claim:
        "Cada claim vincula un actor con un hecho fechado y su rol en él (quién, qué, cuándo, en calidad de qué). Un vínculo entre dos actores es un claim propio.",
    orden_informe:
        "Agrupá las secciones por actor y, dentro de cada actor, en orden temporal ascendente.",
    sesgo_declarado:
        "Prioriza evidencia que nombra actores: los hechos sin actor identificable quedan sub-representados, y un actor nombrado de una forma no prevista queda fuera del recorrido.",
    // Más alto que el general porque la modalidad pide una consulta por cada
    // variante del nombre, cargo y organización de cada actor. Valor
    // provisional hasta que el bench lo mida (pendientes.md #4, Fase 8).
    max_consultas: 30,
};

const CRONOLOGIA: Perfil = Perfil {
    id: "cronologia",
    nombre: "Cronología y crónica de eventos y procesos",
    ejes_pregunta: &[
        "Terminus a quo y ad quem: ¿con qué hecho abre la cronología, y con cuál cierra?",
        "Granularidad: ¿el registro es por día, por mes o por año, y qué se hace con los hechos que la documentación fecha de forma imprecisa?",
        "Evento y contexto: ¿qué cuenta como evento del hilo principal, y qué queda como marco?",
        "Procesos: ¿qué procesos de larga duración hay que seguir a través de los eventos, y en qué momentos se los considera inflexión?",
    ],
    hint_consultas:
        "Combiná los términos del problema con marcadores temporales del recorte —años, meses, nombres de época, hitos conocidos— y consultá los tramos por separado para no dejar períodos ciegos.",
    forma_claim:
        "Cada claim es un evento con su anclaje temporal explícito y, cuando la fuente lo permite, su relación con el evento anterior. Una fecha imprecisa se declara imprecisa, nunca se redondea.",
    orden_informe:
        "Ordená las secciones en orden temporal ascendente y señalá los tramos sin cobertura documental como huecos de la serie.",
    sesgo_declarado:
        "Prioriza evidencia con anclaje temporal: los procesos que las fuentes narran sin fecha quedan sub-representados y la serie puede parecer más continua de lo que el corpus sostiene.",
    max_consultas: 20,
};

const PERFILES: [Perfil; 3] = [GENERAL, TRAYECTORIAS, CRONOLOGIA];

/// Resuelve la modalidad declarada por el job. `None` si no existe.
pub fn resolver(id: &str) -> Option<&'static Perfil> {
    PERFILES.iter().find(|p| p.id == id)
}

/// Perfil general: el que se aplica cuando el job no declara modalidad.
pub fn por_defecto() -> &'static Perfil {
    resolver(ID_POR_DEFECTO).expect("el perfil general está en la tabla")
}

/// Modalidades disponibles, para que la UI las ofrezca sin hardcodearlas.
pub fn catalogo() -> Vec<(&'static str, &'static str)> {
    PERFILES.iter().map(|p| (p.id, p.nombre)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todo_perfil_ofrece_al_menos_cuatro_ejes_de_pregunta() {
        for p in PERFILES {
            assert!(
                p.ejes_pregunta.len() >= 4,
                "{} necesita cuatro ejes para sostener el piso de preguntas",
                p.id
            );
            assert!(p.ejes_pregunta.iter().all(|e| !e.trim().is_empty()));
        }
    }

    #[test]
    fn todo_perfil_declara_su_sesgo() {
        for p in PERFILES {
            assert!(
                !p.sesgo_declarado.trim().is_empty(),
                "{} no declara sesgo: el informe no podría declarar el instrumento",
                p.id
            );
        }
    }

    #[test]
    fn cada_eje_se_parte_en_titulo_y_pregunta_legible() {
        for p in PERFILES {
            for eje in p.ejes_pregunta {
                let (axis, texto) = eje
                    .split_once(": ")
                    .unwrap_or_else(|| panic!("«{eje}» tiene que separar eje y pregunta con «: »"));
                assert!(!axis.trim().is_empty());
                // El texto viaja solo al informe: tiene que leerse como una
                // pregunta al investigador, no como un fragmento suelto.
                assert!(texto.starts_with('¿'), "«{texto}» no se lee como pregunta");
                assert!(texto.contains('?'), "«{texto}» no cierra la pregunta");
            }
        }
    }

    #[test]
    fn todo_perfil_admite_al_menos_una_consulta() {
        for p in PERFILES {
            assert!(
                p.max_consultas >= 1,
                "{} con tope cero invalidaría todo plan",
                p.id
            );
        }
    }

    #[test]
    fn los_identificadores_son_unicos() {
        let mut ids: Vec<&str> = PERFILES.iter().map(|p| p.id).collect();
        ids.sort_unstable();
        let total = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), total);
    }

    #[test]
    fn modalidad_desconocida_no_resuelve() {
        assert!(resolver("biografia-inventada").is_none());
        assert_eq!(por_defecto().id, "general");
    }

    #[test]
    fn el_catalogo_expone_las_tres_modalidades() {
        let catalogo = catalogo();
        assert_eq!(catalogo.len(), 3);
        assert!(catalogo.iter().any(|(id, _)| *id == "trayectorias"));
        assert!(catalogo.iter().any(|(id, _)| *id == "cronologia"));
    }
}

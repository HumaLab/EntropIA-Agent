//! Grafo de dependencias de stages (PLAN §6.2).
//!
//! Los stages de una investigación forman un DAG (`stage_dependencies`). La
//! ordenación topológica con detección de ciclos es **una función** — la usan
//! el DAG de stages y, si algún día existen, las skills encadenadas.

use std::collections::{HashMap, HashSet, VecDeque};

/// Arista del DAG: `(stage, depende_de)`.
pub type Arista = (String, String);

/// Orden topológico de los nodos, respetando que cada nodo aparece después de
/// sus dependencias. Devuelve `Err` con el ciclo detectado si el grafo no es
/// un DAG.
///
/// El orden resultante es el de ejecución segura: todas las dependencias de un
/// nodo ya terminaron cuando le toca ejecutarse.
pub fn orden_topologico(nodos: &[String], aristas: &[Arista]) -> Result<Vec<String>, Vec<String>> {
    let conjunto: HashSet<&String> = nodos.iter().collect();
    let mut grado: HashMap<String, usize> = nodos.iter().map(|n| (n.clone(), 0)).collect();
    let mut sucesores: HashMap<String, Vec<String>> = HashMap::new();
    for (a, b) in aristas {
        // Solo aristas entre nodos conocidos.
        if !conjunto.contains(a) || !conjunto.contains(b) {
            continue;
        }
        *grado.get_mut(a).unwrap() += 1;
        sucesores.entry(b.clone()).or_default().push(a.clone());
    }

    let mut cola: VecDeque<String> = grado
        .iter()
        .filter(|(_, g)| **g == 0)
        .map(|(n, _)| n.clone())
        .collect();
    let mut orden = Vec::with_capacity(nodos.len());
    while let Some(nodo) = cola.pop_front() {
        orden.push(nodo.clone());
        if let Some(hijos) = sucesores.get(&nodo) {
            for hijo in hijos {
                let g = grado.get_mut(hijo).unwrap();
                *g -= 1;
                if *g == 0 {
                    cola.push_back(hijo.clone());
                }
            }
        }
    }

    if orden.len() == nodos.len() {
        Ok(orden)
    } else {
        // Los nodos que quedaron forman parte de (o dependen de) un ciclo.
        let en_ciclo: Vec<String> = grado
            .iter()
            .filter(|(_, g)| **g > 0)
            .map(|(n, _)| n.clone())
            .collect();
        Err(en_ciclo)
    }
}

/// Dependientes transitivos de un nodo: todos los que (directa o
/// indirectamente) dependen de él. Se usa para invalidar stages cuando un
/// hallazgo tardío rehace una etapa anterior (§6.2).
pub fn dependientes_transitivos(nodos: &[String], aristas: &[Arista], raiz: &str) -> Vec<String> {
    let conjunto: HashSet<&String> = nodos.iter().collect();
    let mut dependientes: HashMap<String, Vec<String>> = HashMap::new();
    for (a, b) in aristas {
        // `a` depende de `b`: `b` → `a`.
        if !conjunto.contains(a) || !conjunto.contains(b) {
            continue;
        }
        dependientes.entry(b.clone()).or_default().push(a.clone());
    }
    let mut vistos: HashSet<String> = HashSet::new();
    let mut pila = vec![raiz.to_string()];
    while let Some(nodo) = pila.pop() {
        if let Some(hijos) = dependientes.get(&nodo) {
            for hijo in hijos {
                if vistos.insert(hijo.clone()) {
                    pila.push(hijo.clone());
                }
            }
        }
    }
    vistos.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(nombres: &[&str]) -> Vec<String> {
        nombres.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn ordena_respetando_dependencias() {
        // a → b → c (c depende de b, b de a) y d independiente.
        let nodos = ids(&["a", "b", "c", "d"]);
        let aristas = vec![
            ("b".to_string(), "a".to_string()),
            ("c".to_string(), "b".to_string()),
        ];
        let orden = orden_topologico(&nodos, &aristas).unwrap();
        let pos: HashMap<&str, usize> = orden
            .iter()
            .enumerate()
            .map(|(i, n)| (n.as_str(), i))
            .collect();
        assert!(pos["a"] < pos["b"]);
        assert!(pos["b"] < pos["c"]);
    }

    #[test]
    fn detecta_ciclos() {
        let nodos = ids(&["a", "b", "c"]);
        let aristas = vec![
            ("b".to_string(), "a".to_string()),
            ("c".to_string(), "b".to_string()),
            ("a".to_string(), "c".to_string()),
        ];
        let err = orden_topologico(&nodos, &aristas).unwrap_err();
        assert_eq!(err.len(), 3);
    }

    #[test]
    fn ignora_aristas_a_nodos_desconocidos() {
        let nodos = ids(&["a", "b"]);
        let aristas = vec![
            ("b".to_string(), "a".to_string()),
            ("x".to_string(), "a".to_string()),
        ];
        assert!(orden_topologico(&nodos, &aristas).is_ok());
    }

    #[test]
    fn dependientes_transitivos_cubren_la_cadena() {
        let nodos = ids(&["a", "b", "c", "d"]);
        let aristas = vec![
            ("b".to_string(), "a".to_string()),
            ("c".to_string(), "b".to_string()),
            ("d".to_string(), "c".to_string()),
        ];
        let mut dep = dependientes_transitivos(&nodos, &aristas, "a");
        dep.sort();
        assert_eq!(dep, ids(&["b", "c", "d"]));
    }

    #[test]
    fn dependientes_transitivos_rama_independiente() {
        let nodos = ids(&["a", "b", "x"]);
        let aristas = vec![
            ("b".to_string(), "a".to_string()),
            ("x".to_string(), "b".to_string()),
        ];
        let dep = dependientes_transitivos(&nodos, &aristas, "b");
        assert_eq!(dep, ids(&["x"]));
    }
}

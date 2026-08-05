//! EntropIA-Agent: motor de investigación historiográfica sobre el corpus de
//! EntropIA.
//!
//! Crate lib + bin (PLAN §2, Fase 0). La lib expone el núcleo que se integra
//! en EntropIA Lite/Pro como solapa Tauri; el bin `entropia-agent` es el
//! harness delgado de desarrollo y prueba.
//!
//! Frontera de confianza (§2 del PLAN): el agente **solo lee** las tablas de
//! procesamiento del corpus (`rag_chunks`, `items`, `collections`, …). El
//! estado propio del agente vive en `estado.sqlite` (escritura), creado en
//! Fase 1.

pub mod agente;
pub mod cliente_llm;
pub mod configuracion;
pub mod embeddings;
pub mod estado;
pub mod grafo;
pub mod informe;
pub mod memoria;
pub mod prompts;
pub mod recuperacion;
pub mod repositorio;
pub mod rerank;
pub mod trabajos;
pub mod vector;

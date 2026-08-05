//! Operaciones vectoriales para la búsqueda semántica.
//!
//! Réplica del contrato del desktop de EntropIA: los embeddings de la base
//! son f32 little-endian contiguos (1024 dimensiones) y la similitud se
//! calcula con coseno en doble precisión.

/// Decodifica un BLOB de embedding (f32 little-endian) en un vector.
pub fn decodificar_embedding(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Normaliza un vector a longitud 1 (norma L2).
pub fn normalizar(v: &[f32]) -> Vec<f32> {
    let magnitud: f64 = v.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    if magnitud == 0.0 {
        return v.to_vec();
    }
    v.iter().map(|x| (*x as f64 / magnitud) as f32).collect()
}

/// Similitud coseno entre dos vectores. Devuelve 0 si las longitudes no
/// coinciden o algún vector es nulo.
pub fn similitud_coseno(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let producto: f64 = a
        .iter()
        .zip(b.iter())
        .map(|(x, y)| (*x as f64) * (*y as f64))
        .sum();
    let mag_a: f64 = a.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    let mag_b: f64 = b.iter().map(|y| (*y as f64).powi(2)).sum::<f64>().sqrt();
    if mag_a == 0.0 || mag_b == 0.0 {
        return 0.0;
    }
    (producto / (mag_a * mag_b)) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodifica_embedding_little_endian() {
        // 0x3F800000 es 1.0f32 en little-endian.
        let blob = [0x00u8, 0x00, 0x80, 0x3F];
        let v = decodificar_embedding(&blob);
        assert_eq!(v, vec![1.0]);
    }

    #[test]
    fn normaliza_a_longitud_uno() {
        let v = normalizar(&[3.0, 4.0]);
        let mag: f64 = v.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
        assert!((mag - 1.0).abs() < 1e-6);
    }

    #[test]
    fn coseno_de_vectores_iguales_es_uno() {
        assert!((similitud_coseno(&[1.0, 2.0], &[1.0, 2.0]) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn coseno_de_vectores_ortogonales_es_cero() {
        assert!(similitud_coseno(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
    }

    #[test]
    fn coseno_de_vectores_opuestos_es_menos_uno() {
        assert!((similitud_coseno(&[1.0], &[-1.0]) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn coseno_de_vector_nulo_es_cero() {
        assert_eq!(similitud_coseno(&[0.0, 0.0], &[1.0, 2.0]), 0.0);
    }

    #[test]
    fn coseno_de_longitudes_distintas_es_cero() {
        assert_eq!(similitud_coseno(&[1.0], &[1.0, 2.0]), 0.0);
    }
}

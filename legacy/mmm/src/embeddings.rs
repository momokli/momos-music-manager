// src/embeddings.rs
//
// Semantische Embeddings für Music Tags mit candle (all-MiniLM-L6-v2)
//
// Ablauf:
//   1. Modell + Tokenizer von HuggingFace laden (lazy, beim ersten Bedarf)
//   2. Tag-Strings vektorisieren → 384-dim f32 Vektor
//   3. Cosine Similarity zwischen Vektoren
//   4. Category-Embedding = Mittelwert aller Tag-Embeddings dieser Kategorie
//   5. AI-Suggestion: Tag ↔ alle Categories (exkl. Setlist) vergleichen

use anyhow::{Context, Result};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config, DTYPE};
use hf_hub::api::sync::Api;
use std::collections::HashMap;

use tokenizers::Tokenizer;

// ─── Embedding Model ──────────────────────────────────────────────────────────

pub struct EmbeddingModel {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
    #[allow(dead_code)]
    pub dim: usize,
}

impl EmbeddingModel {
    /// Lädt all-MiniLM-L6-v2 von HuggingFace (via hf-hub Cache)
    pub fn new() -> Result<Self> {
        let device = Device::Cpu;

        let api = Api::new().context("Failed to init hf-hub API")?;
        let repo = api.model("sentence-transformers/all-MiniLM-L6-v2".to_string());

        let config_path = repo
            .get("config.json")
            .context("config.json not found in model repo")?;
        let tokenizer_path = repo
            .get("tokenizer.json")
            .context("tokenizer.json not found in model repo")?;
        let model_path = repo
            .get("model.safetensors")
            .context("model.safetensors not found in model repo")?;

        // Config parsen
        let config_content =
            std::fs::read_to_string(&config_path).context("Failed to read config.json")?;
        let config: Config =
            serde_json::from_str(&config_content).context("Failed to parse config.json")?;

        let dim = config.hidden_size;

        // Tokenizer laden
        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Tokenizer load error: {}", e))
            .context("Failed to load tokenizer")?;

        // Modellgewichte laden (safetensors)
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[model_path], DTYPE, &device)
                .context("Failed to load safetensors")?
        };

        let model = BertModel::load(vb, &config).context("Failed to create BertModel")?;

        tracing::info!("Model loaded: all-MiniLM-L6-v2 ({} dim)", dim);

        Ok(Self {
            model,
            tokenizer,
            device,
            dim,
        })
    }

    /// Berechnet ein Embedding für einen Text-String
    ///
    /// 1. Tokenize (truncate/pad auf max 128 Token)
    /// 2. Forward pass durch BERT
    /// 3. Mean Pooling (über alle Token, gewichtet mit attention_mask)
    /// 4. L2-Normalisierung
    pub fn embed_text(&self, text: &str) -> Result<Vec<f32>> {
        // Tokenize
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| anyhow::anyhow!("Tokenizer encode error: {}", e))?;

        let input_ids = encoding.get_ids();
        let attention_mask = encoding.get_attention_mask();
        let token_type_ids = encoding.get_type_ids();

        // Truncate/Padding auf 128 Token (all-MiniLM-L6-v2 kann bis 512)
        let max_len = 128usize;
        let pad_id = 0u32;

        let input_ids: Vec<u32> = if input_ids.len() > max_len {
            input_ids[..max_len].to_vec()
        } else {
            let mut padded = input_ids.to_vec();
            padded.resize(max_len, pad_id);
            padded
        };

        let attention_mask: Vec<u32> = if attention_mask.len() > max_len {
            attention_mask[..max_len].to_vec()
        } else {
            let mut padded = attention_mask.to_vec();
            padded.resize(max_len, 0u32);
            padded
        };

        let token_type_ids: Vec<u32> = if token_type_ids.len() > max_len {
            token_type_ids[..max_len].to_vec()
        } else {
            let mut padded = token_type_ids.to_vec();
            padded.resize(max_len, 0u32);
            padded
        };

        // Tensoren erstellen: shape (1, seq_len)
        let seq_len = input_ids.len();
        let input_ids = Tensor::from_slice(&input_ids, (1, seq_len), &self.device)?;
        let attention_mask = Tensor::from_slice(&attention_mask, (1, seq_len), &self.device)?;
        let token_type_ids = Tensor::from_slice(&token_type_ids, (1, seq_len), &self.device)?;

        // Forward pass
        // Output shape: (1, seq_len, hidden_size)
        let output = self
            .model
            .forward(&input_ids, &token_type_ids, Some(&attention_mask))?;

        // Mean Pooling: Summe über seq_len-Dimension / Anzahl der Attention-Token
        // attention_mask auf f32 casten für mathematische Operationen
        let mask = attention_mask.unsqueeze(2)?.to_dtype(DType::F32)?; // (1, seq_len, 1)
        let masked_output = output.broadcast_mul(&mask)?; // (1, seq_len, hidden)
        let sum = masked_output.sum(1)?; // (1, hidden)
        let token_count = mask.sum(1)?; // (1, 1)
        // Vermeide Division durch Null (beide in Shape [1, 1])
        let min_count = Tensor::from_slice(&[1f32], (1, 1), &self.device)?;
        let token_count = token_count.maximum(&min_count)?;
        let mean = sum.broadcast_div(&token_count)?; // (1, hidden)

        // L2-Normalisierung
        let norm = mean.sqr()?.sum(1)?.sqrt()?; // (1,) — sum(1) auf 2D-Tensor gibt 1D
        let norm = norm.maximum(&Tensor::new(&[1e-12f32], &self.device)?)?;
        let normalized = mean.broadcast_div(&norm)?;

        // Vektor aus Tensor extrahieren
        let vec: Vec<f32> = normalized.squeeze(0)?.to_vec1()?;

        Ok(vec)
    }
}

// ─── Similarity Functions ─────────────────────────────────────────────────────

/// Cosine Similarity zwischen zwei f32 slices
/// Ergebnis: -1.0 bis 1.0 (beide müssen gleiche Länge haben)
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a * norm_b)).clamp(-1.0, 1.0)
}

/// Berechnet das Mean-Embedding einer Liste von Vektoren
pub fn mean_embedding(vectors: &[Vec<f32>]) -> Vec<f32> {
    if vectors.is_empty() {
        return vec![];
    }
    let dim = vectors[0].len();
    let mut mean = vec![0.0f32; dim];
    for v in vectors {
        for (i, &val) in v.iter().enumerate() {
            mean[i] += val;
        }
    }
    let count = vectors.len() as f32;
    for val in &mut mean {
        *val /= count;
    }
    mean
}

/// Normalisiert einen Vektor auf L2-Länge 1 (in-place)
#[allow(dead_code)]
pub fn l2_normalize(vec: &mut [f32]) {
    let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-12 {
        for val in vec.iter_mut() {
            *val /= norm;
        }
    }
}

// ─── Serialisierung ───────────────────────────────────────────────────────────

/// Serialisiert Vec<f32> → BLOB (für SQLite Speicherung)
pub fn serialize_embedding(vec: &[f32]) -> Vec<u8> {
    let bytes: Vec<u8> = vec.iter().flat_map(|f| f.to_le_bytes()).collect();
    bytes
}

/// Deserialisiert BLOB → Vec<f32>
pub fn deserialize_embedding(blob: &[u8]) -> Result<Vec<f32>> {
    if blob.len() % 4 != 0 {
        anyhow::bail!(
            "Invalid embedding blob length: {} (must be multiple of 4)",
            blob.len()
        );
    }
    let vec: Vec<f32> = blob
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();
    Ok(vec)
}

// ─── Kategorie-Suggestion ─────────────────────────────────────────────────────

/// Ergebnis einer Kategorie-Empfehlung
#[derive(Debug, Clone)]
pub struct CategorySuggestion {
    pub category_id: i64,
    pub category_name: String,
    pub confidence: f32,
}

/// Findet die beste Kategorie für ein Tag-Embedding
/// Vergleicht mittels Cosine Similarity gegen alle Category-Embeddings
/// `category_embeddings`: Map von category_id → Mean-Embedding der Tags in dieser Cat.
/// `skip_category_id`: Kategorie-ID die ignoriert werden soll (z.B. Setlist/Default)
pub fn suggest_category(
    tag_embedding: &[f32],
    category_embeddings: &HashMap<i64, (String, Vec<f32>)>,
    skip_category_id: i64,
) -> Option<CategorySuggestion> {
    let mut best: Option<CategorySuggestion> = None;

    for (&cat_id, (cat_name, cat_embedding)) in category_embeddings {
        if cat_id == skip_category_id {
            continue;
        }
        let score = cosine_similarity(tag_embedding, cat_embedding);
        let is_better = match &best {
            None => true,
            Some(current) => score > current.confidence,
        };
        if is_better {
            best = Some(CategorySuggestion {
                category_id: cat_id,
                category_name: cat_name.clone(),
                confidence: score,
            });
        }
    }

    best
}

// ─── Tag Similarities (Batch Compute) ─────────────────────────────────────────

/// Compute pairwise cosine similarity for all tag embeddings and persist to DB.
///
/// 1. Fetch all (tag_id, embedding_blob) from tag_embeddings
/// 2. Deserialize all embeddings
/// 3. Compute cosine similarity for each unique pair (i, j) where i < j
/// 4. Batch INSERT into tag_similarities table using a transaction
///
/// Returns the number of similarity pairs computed.
/// Skips tags without embeddings. Clears old similarities before computing.
pub async fn compute_tag_similarities(pool: &sqlx::Pool<sqlx::Sqlite>) -> Result<usize> {
    use sqlx::Row;

    // Fetch all tag embeddings
    let rows =
        sqlx::query("SELECT te.tag_id, te.embedding FROM tag_embeddings te ORDER BY te.tag_id")
            .fetch_all(pool)
            .await?;

    let mut tag_ids: Vec<i64> = Vec::new();
    let mut embeddings: Vec<Vec<f32>> = Vec::new();

    for row in &rows {
        let tag_id: i64 = row.try_get("tag_id")?;
        let blob: Vec<u8> = row.try_get("embedding")?;
        match deserialize_embedding(&blob) {
            Ok(vec) => {
                tag_ids.push(tag_id);
                embeddings.push(vec);
            }
            Err(e) => {
                tracing::warn!("Failed to deserialize embedding for tag {}: {}", tag_id, e);
            }
        }
    }

    let n = tag_ids.len();
    if n < 2 {
        tracing::info!(
            "Need at least 2 tags with embeddings to compute similarities (found {})",
            n
        );
        return Ok(0);
    }

    // Clear old similarities
    sqlx::query("DELETE FROM tag_similarities")
        .execute(pool)
        .await?;

    let now = chrono::Utc::now().timestamp();
    let mut count = 0usize;

    // Batch inserts in chunks of 500 per transaction to avoid holding
    // the SQLite write lock for too long (~88k pairs for ~420 tags).
    const BATCH_SIZE: usize = 500;
    let mut tx = pool.begin().await?;

    for i in 0..n {
        for j in (i + 1)..n {
            let sim = cosine_similarity(&embeddings[i], &embeddings[j]);
            // Only store pairs with similarity > 0.1 (below that is noise)
            if sim > 0.1 {
                sqlx::query(
                    "INSERT INTO tag_similarities (tag_a_id, tag_b_id, similarity, updated_at) VALUES (?, ?, ?, ?)"
                )
                .bind(tag_ids[i])
                .bind(tag_ids[j])
                .bind(sim)
                .bind(now)
                .execute(&mut *tx)
                .await?;
                count += 1;

                // Commit every BATCH_SIZE inserts to limit lock duration
                if count % BATCH_SIZE == 0 {
                    tx.commit().await?;
                    tx = pool.begin().await?;
                }
            }
        }
    }

    // Commit the final batch (fewer than BATCH_SIZE rows, if any)
    tx.commit().await?;

    tracing::info!("Computed {} tag similarity pairs for {} tags", count, n);
    Ok(count)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_mean_embedding() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![3.0, 2.0, 1.0];
        let mean = mean_embedding(&[a, b]);
        assert!((mean[0] - 2.0).abs() < 1e-6);
        assert!((mean[1] - 2.0).abs() < 1e-6);
        assert!((mean[2] - 2.0).abs() < 1e-6);
    }

    #[test]
    fn test_serialize_roundtrip() {
        let original = vec![1.0, -2.5, 42.0, 0.0, 1e-10];
        let blob = serialize_embedding(&original);
        let deserialized = deserialize_embedding(&blob).unwrap();
        assert_eq!(original.len(), deserialized.len());
        for (a, b) in original.iter().zip(deserialized.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn test_suggest_category_skip() {
        let tag_emb = vec![1.0, 0.0, 0.0];
        let mut cats: HashMap<i64, (String, Vec<f32>)> = HashMap::new();
        cats.insert(1, ("Setlist".into(), vec![1.0, 1.0, 0.0]));
        cats.insert(2, ("Mood".into(), vec![1.0, 0.0, 0.0]));
        cats.insert(3, ("Phase".into(), vec![0.5, 0.5, 0.0]));

        // Skip Setlist (id=1) → Mood sollte gewinnen
        let result = suggest_category(&tag_emb, &cats, 1);
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.category_name, "Mood");
        assert!(r.confidence > 0.99);
    }

    #[test]
    fn test_l2_normalize() {
        let mut v = vec![3.0, 4.0]; // norm = 5
        l2_normalize(&mut v);
        assert!((v[0] - 0.6).abs() < 1e-6);
        assert!((v[1] - 0.8).abs() < 1e-6);
    }
}

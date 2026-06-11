use async_trait::async_trait;
use serde::Deserialize;

use super::{AiError, AiResult};
use crate::net::{EgressFeature, GuardedClient};

/// Провайдер эмбеддингов (**ADR-005**): отдельная сущность от chat. `query`/`document` —
/// асимметрия задач (nomic/bge требуют разные префиксы); L2-нормализация — внутри.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Эмбеддит документы (1:1 к входу). Под капотом — document-префикс + L2.
    async fn embed_documents(&self, texts: &[&str]) -> AiResult<Vec<Vec<f32>>>;
    /// Эмбеддит поисковый запрос (query-префикс + L2).
    async fn embed_query(&self, text: &str) -> AiResult<Vec<f32>>;
    /// Размерность вектора (ИЗ модели, не хардкод — §5/§6.5).
    fn dim(&self) -> usize;
    /// Идентификатор модели (для инвалидации векторов при смене — §6.5).
    fn model_id(&self) -> &str;
}

/// L2-нормализация на месте (идемпотентна; страхует, если сервер не нормализует).
pub fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Task-префиксы по идентификатору модели (nomic/e5 требуют `search_query:`/`search_document:`;
/// для bge-m3 и прочих — пусто). Эвристика по подстроке имени — настраиваемо позже через конфиг.
pub fn default_prefixes(model: &str) -> Option<(String, String)> {
    let m = model.to_lowercase();
    if m.contains("nomic") {
        Some(("search_query: ".into(), "search_document: ".into()))
    } else if m.contains("e5") {
        Some(("query: ".into(), "passage: ".into()))
    } else {
        None
    }
}

/// Эмбеддер через OpenAI-совместимый `POST {base}/v1/embeddings` (llama.cpp-server).
/// Применяет task-префиксы (nomic: `search_query:` / `search_document:`) и L2-нормализацию.
pub struct OpenAiEmbedder {
    /// Guarded-клиент ядра (ADR-005-ext): политика+audit на каждый запрос (AC-EGR-1/6).
    client: GuardedClient,
    /// Feature-тег эгресса — задаёт composition-root (обычно [`EgressFeature::Embed`]).
    feature: EgressFeature,
    endpoint: String,
    model: String,
    dim: usize,
    query_prefix: String,
    document_prefix: String,
}

#[derive(Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingItem>,
}

#[derive(Deserialize)]
struct EmbeddingItem {
    embedding: Vec<f32>,
}

impl OpenAiEmbedder {
    /// `prefixes` = `(query_prefix, document_prefix)` (для nomic; для моделей без префиксов — `None`).
    /// Таймауты — у guarded-клиента (профиль [`GuardedClient::for_embedding`], 60 с).
    pub fn new(
        client: &GuardedClient,
        feature: EgressFeature,
        base_url: &str,
        model: &str,
        dim: usize,
        prefixes: Option<(String, String)>,
    ) -> Self {
        let (query_prefix, document_prefix) = prefixes.unwrap_or_default();
        Self {
            client: client.clone(),
            feature,
            endpoint: format!("{}/v1/embeddings", crate::ai::api_base(base_url)),
            model: model.to_string(),
            dim,
            query_prefix,
            document_prefix,
        }
    }

    /// Узнаёт размерность модели одним пробным эмбеддингом (когда `embedding.dim` не задан
    /// в `local.json`). Не применяет проверку/префиксы — только длину вектора. Через guarded
    /// с [`EgressFeature::Probe`] (AC-EGR-6): url вне политики → `Denied` ДО сети.
    pub async fn probe_dim(client: &GuardedClient, base_url: &str, model: &str) -> AiResult<usize> {
        let endpoint = format!("{}/v1/embeddings", crate::ai::api_base(base_url));
        let body = serde_json::json!({ "model": model, "input": ["dim probe"] });
        let resp = client
            .post_json(&endpoint, EgressFeature::Probe, &body)
            .await
            .map_err(AiError::from)?;
        if !resp.status().is_success() {
            return Err(AiError::Http(format!("статус {}", resp.status())));
        }
        let parsed: EmbeddingsResponse = resp
            .json()
            .await
            .map_err(|e| AiError::BadResponse(e.to_string()))?;
        parsed
            .data
            .first()
            .map(|i| i.embedding.len())
            .filter(|&n| n > 0)
            .ok_or_else(|| AiError::BadResponse("пустой ответ при пробе размерности".into()))
    }

    async fn embed_raw(&self, inputs: Vec<String>) -> AiResult<Vec<Vec<f32>>> {
        let body = serde_json::json!({ "model": self.model, "input": inputs });
        let resp = self
            .client
            .post_json(&self.endpoint, self.feature, &body)
            .await
            .map_err(AiError::from)?;
        if !resp.status().is_success() {
            return Err(AiError::Http(format!("статус {}", resp.status())));
        }
        let parsed: EmbeddingsResponse = resp
            .json()
            .await
            .map_err(|e| AiError::BadResponse(e.to_string()))?;
        let mut out = Vec::with_capacity(parsed.data.len());
        for item in parsed.data {
            let mut v = item.embedding;
            if v.len() != self.dim {
                return Err(AiError::DimMismatch {
                    expected: self.dim,
                    got: v.len(),
                });
            }
            l2_normalize(&mut v);
            out.push(v);
        }
        Ok(out)
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAiEmbedder {
    async fn embed_documents(&self, texts: &[&str]) -> AiResult<Vec<Vec<f32>>> {
        let inputs = texts
            .iter()
            .map(|t| format!("{}{}", self.document_prefix, t))
            .collect();
        self.embed_raw(inputs).await
    }

    async fn embed_query(&self, text: &str) -> AiResult<Vec<f32>> {
        let input = format!("{}{}", self.query_prefix, text);
        self.embed_raw(vec![input])
            .await?
            .pop()
            .ok_or_else(|| AiError::BadResponse("пустой ответ эмбеддера".into()))
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn model_id(&self) -> &str {
        &self.model
    }
}

/// Детерминированный мок-эмбеддер для тестов БЕЗ сервера (Ф1-4/5/6).
#[cfg(test)]
pub(crate) struct MockEmbedder {
    pub dim: usize,
}

#[cfg(test)]
fn mock_vec(text: &str, dim: usize) -> Vec<f32> {
    let mut v = vec![0f32; dim];
    for (i, b) in text.bytes().enumerate() {
        v[i % dim] += f32::from(b) / 255.0;
    }
    l2_normalize(&mut v);
    v
}

#[cfg(test)]
#[async_trait]
impl EmbeddingProvider for MockEmbedder {
    async fn embed_documents(&self, texts: &[&str]) -> AiResult<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| mock_vec(t, self.dim)).collect())
    }
    async fn embed_query(&self, text: &str) -> AiResult<Vec<f32>> {
        Ok(mock_vec(text, self.dim))
    }
    fn dim(&self) -> usize {
        self.dim
    }
    fn model_id(&self) -> &str {
        "mock"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }

    #[test]
    fn l2_normalize_makes_unit_norm() {
        let mut v = vec![3.0, 4.0];
        l2_normalize(&mut v);
        let norm = (v[0] * v[0] + v[1] * v[1]).sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
        // нулевой вектор не делится на ноль
        let mut z = vec![0.0, 0.0];
        l2_normalize(&mut z);
        assert_eq!(z, vec![0.0, 0.0]);
    }

    #[tokio::test]
    async fn mock_embedder_is_deterministic_and_normalized() {
        let e = MockEmbedder { dim: 16 };
        let a = e.embed_query("hello").await.unwrap();
        let b = e.embed_query("hello").await.unwrap();
        assert_eq!(a, b, "детерминирован");
        assert_eq!(a.len(), 16);
        assert!((cosine(&a, &a) - 1.0).abs() < 1e-5, "нормализован");
        let c = e.embed_query("different").await.unwrap();
        assert_ne!(a, c);
    }

    /// AC-EGR-6: `probe_dim` идёт через guarded с `Feature::Probe` — отказ политики (выключенный
    /// Probe-opt-in) возвращается ТИПИЗИРОВАННЫМ `AiError::Denied` ДО любых сетевых действий.
    #[tokio::test]
    async fn probe_dim_is_guarded() {
        use crate::net::{EgressAudit, EgressDenied, EgressPolicy};
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;

        let policy = Arc::new(EgressPolicy::new(Arc::new(AtomicBool::new(false))));
        policy.set_feature_enabled(EgressFeature::Probe, false);
        let guarded = GuardedClient::new(policy, Arc::new(EgressAudit::default()), |b| b).unwrap();
        let res = OpenAiEmbedder::probe_dim(&guarded, "http://127.0.0.1:9", "m").await;
        assert!(
            matches!(
                res,
                Err(AiError::Denied(EgressDenied::FeatureNotEnabled(
                    EgressFeature::Probe
                )))
            ),
            "ожидали типизированный отказ политики: {res:?}"
        );
    }

    /// Живой smoke против прод-эмбеддера bge-m3 (запуск: `cargo test -- --ignored`;
    /// `NEXUS_EMBED_URL` — оверрайд хоста). В CI пропускается.
    #[tokio::test]
    #[ignore = "нужен embedding-сервер (NEXUS_EMBED_URL, default 192.168.0.31:8083)"]
    async fn live_embedder_embeds_and_ranks_semantically() {
        let url =
            std::env::var("NEXUS_EMBED_URL").unwrap_or_else(|_| "http://192.168.0.31:8083".into());
        let e = OpenAiEmbedder::new(
            &GuardedClient::unchecked(),
            EgressFeature::Embed,
            &url,
            "bge-m3",
            1024,
            default_prefixes("bge-m3"),
        );

        let docs = e
            .embed_documents(&[
                "кошка сидит на тёплом коврике у окна",
                "квантовая хромодинамика и сильное взаимодействие кварков",
            ])
            .await
            .unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].len(), 1024);
        let norm = docs[0].iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3);

        let q = e.embed_query("где находится кошка?").await.unwrap();
        assert!(
            cosine(&q, &docs[0]) > cosine(&q, &docs[1]),
            "запрос про кошку должен быть ближе к doc про кошку, чем к doc про физику"
        );
    }
}

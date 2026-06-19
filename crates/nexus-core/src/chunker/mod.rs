//! Markdown-чанкер (§6.1): режет тело на чанки для RAG-эмбеддинга.
//!
//! Стратегия: разбиение по заголовкам → если секция влезает, она и есть чанк; иначе sliding
//! window с overlap ВНУТРИ окна (хвост предыдущего повторяется, а не добавляется СВЕРХ лимита —
//! иначе чанк > бюджета). Frontmatter вырезан (в тело не попадает). Fenced-code атомарен (не
//! рвётся посреди окна). `token_count` — по финальному содержимому каждого чанка.
//!
//! Токены считает [`Tokenizer`]; пока — эвристика [`WordTokenizer`] (placeholder). Реальный
//! токенайзер эмбеддера подключится в Ф1-3 (для кириллицы эвристика по символам врёт в 1.5–2×).

use crate::parser::split_frontmatter;

/// Считает «токены» текста. Реализация эмбеддера придёт в Ф1-3.
pub trait Tokenizer: Send + Sync {
    fn count(&self, text: &str) -> usize;
}

/// Эвристический токенайзер-placeholder: число слов (детерминирован, без зависимости от модели).
pub struct WordTokenizer;

impl Tokenizer for WordTokenizer {
    fn count(&self, text: &str) -> usize {
        text.split_whitespace().count()
    }
}

/// Параметры чанкинга. `max_tokens` — ВКЛЮЧАЯ overlap.
#[derive(Debug, Clone, Copy)]
pub struct ChunkOptions {
    pub max_tokens: usize,
    pub overlap_tokens: usize,
}

impl Default for ChunkOptions {
    fn default() -> Self {
        Self {
            max_tokens: 512,
            overlap_tokens: 64,
        }
    }
}

/// Готовый чанк (1:1 к строке таблицы `chunks`, `file_id` добавляется при вставке).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub chunk_index: usize,
    pub content: String,
    /// Смещения в ИСХОДНОМ файле (с учётом frontmatter) — для dedup overlap и jump-to-section.
    pub char_start: usize,
    pub char_end: usize,
    pub heading_path: Option<String>,
    pub token_count: usize,
}

/// Атомарный блок внутри секции (строка ИЛИ цельный fenced-code). Смещения — в теле.
struct Block {
    start: usize,
    end: usize,
    tokens: usize,
}

/// Разбивает документ на чанки.
pub fn chunk_document(content: &str, tokenizer: &dyn Tokenizer, opts: ChunkOptions) -> Vec<Chunk> {
    let (_, body, _) = split_frontmatter(content);
    let body_offset = content.len() - body.len();

    let mut out = Vec::new();
    let mut index = 0usize;
    for (start, end, heading_path) in split_sections(body) {
        let blocks = blocks_of(body, start, end, tokenizer);
        window_chunks(
            &blocks,
            body,
            body_offset,
            &opts,
            heading_path.as_deref(),
            tokenizer,
            &mut out,
            &mut index,
        );
    }
    out
}

/// Уровень ATX-заголовка (`# ` … `###### `), иначе `None`.
fn atx_level(line: &str) -> Option<usize> {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    let after = line.as_bytes().get(hashes).copied();
    if (1..=6).contains(&hashes) && matches!(after, None | Some(b' ') | Some(b'\t')) {
        Some(hashes)
    } else {
        None
    }
}

/// Делит тело на секции по заголовкам; каждая секция — `(start, end, heading_path)` (offsets в теле).
/// heading_path — стек предков, напр. `"H1 > H2"`. Заголовки внутри fenced-code игнорируются.
fn split_sections(body: &str) -> Vec<(usize, usize, Option<String>)> {
    let mut sections = Vec::new();
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut cur_start = 0usize;
    let mut cur_path: Option<String> = None;
    let mut offset = 0usize;
    let mut in_fence = false;

    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.trim_end().starts_with("```") {
            in_fence = !in_fence;
        } else if !in_fence {
            if let Some(level) = atx_level(trimmed) {
                if offset > cur_start {
                    sections.push((cur_start, offset, cur_path.clone()));
                }
                let title = trimmed[level..]
                    .trim()
                    .trim_end_matches('#')
                    .trim()
                    .to_string();
                while matches!(stack.last(), Some((l, _)) if *l >= level) {
                    stack.pop();
                }
                stack.push((level, title));
                cur_path = Some(
                    stack
                        .iter()
                        .map(|(_, t)| t.as_str())
                        .collect::<Vec<_>>()
                        .join(" > "),
                );
                cur_start = offset;
            }
        }
        offset += line.len();
    }
    if offset > cur_start {
        sections.push((cur_start, offset, cur_path));
    }
    sections
}

/// Атомарные единицы секции `[start, end)` для упаковки: каждое СЛОВО прозы — единица,
/// каждый fenced-code — одна цельная единица (не рвётся). Так sliding window получает
/// overlap по словам, а код остаётся атомарным.
fn blocks_of(body: &str, start: usize, end: usize, tokenizer: &dyn Tokenizer) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut off = start;
    let mut fence_start: Option<usize> = None;

    for line in body[start..end].split_inclusive('\n') {
        let line_end = off + line.len();
        let is_fence = line.trim_start().trim_end().starts_with("```");
        match (fence_start, is_fence) {
            (None, true) => fence_start = Some(off),
            (Some(fs), true) => {
                blocks.push(unit(body, fs, line_end, tokenizer));
                fence_start = None;
            }
            (Some(_), false) => {} // строка внутри fenced-code — копится в один блок
            (None, false) => {
                for (ws, we) in word_ranges(&body[off..line_end]) {
                    blocks.push(unit(body, off + ws, off + we, tokenizer));
                }
            }
        }
        off = line_end;
    }
    if let Some(fs) = fence_start {
        blocks.push(unit(body, fs, end, tokenizer));
    }
    blocks
}

fn unit(body: &str, start: usize, end: usize, tokenizer: &dyn Tokenizer) -> Block {
    Block {
        start,
        end,
        tokens: tokenizer.count(&body[start..end]),
    }
}

/// Байтовые диапазоны слов (разделители — ASCII-пробелы; многобайтовые символы — внутри слов).
fn word_ranges(s: &str) -> Vec<(usize, usize)> {
    let b = s.as_bytes();
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let start = i;
        while i < b.len() && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        ranges.push((start, i));
    }
    ranges
}

/// Пакует блоки в окна ≤ `max_tokens`; начало следующего окна сдвигается назад на
/// `overlap_tokens` (хвост повторяется ВНУТРИ окна). Один блок крупнее лимита — отдельное окно.
#[allow(clippy::too_many_arguments)]
fn window_chunks(
    blocks: &[Block],
    body: &str,
    body_offset: usize,
    opts: &ChunkOptions,
    heading_path: Option<&str>,
    tokenizer: &dyn Tokenizer,
    out: &mut Vec<Chunk>,
    index: &mut usize,
) {
    let n = blocks.len();
    if n == 0 {
        return;
    }
    let mut i = 0;
    loop {
        let mut j = i;
        let mut tok = 0;
        while j < n && (j == i || tok + blocks[j].tokens <= opts.max_tokens) {
            tok += blocks[j].tokens;
            j += 1;
        }

        let start = blocks[i].start;
        let end = blocks[j - 1].end;
        let text = body[start..end].trim_end();
        out.push(Chunk {
            chunk_index: *index,
            content: text.to_string(),
            char_start: body_offset + start,
            char_end: body_offset + start + text.len(),
            heading_path: heading_path.map(str::to_owned),
            token_count: tokenizer.count(text),
        });
        *index += 1;

        if j >= n {
            break;
        }

        // Сдвиг назад на overlap_tokens (повтор хвоста), с гарантией прогресса.
        let mut k = j;
        let mut otok = 0;
        while k > i + 1 && otok + blocks[k - 1].tokens <= opts.overlap_tokens {
            otok += blocks[k - 1].tokens;
            k -= 1;
        }
        i = if k > i { k } else { i + 1 };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(max: usize, overlap: usize) -> ChunkOptions {
        ChunkOptions {
            max_tokens: max,
            overlap_tokens: overlap,
        }
    }

    #[test]
    fn short_doc_single_chunk_no_heading() {
        let chunks = chunk_document(
            "just a few words here",
            &WordTokenizer,
            ChunkOptions::default(),
        );
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].heading_path, None);
        assert_eq!(chunks[0].char_start, 0);
        assert_eq!(chunks[0].token_count, 5);
        assert_eq!(chunks[0].content, "just a few words here");
    }

    #[test]
    fn frontmatter_excluded_and_offsets_shifted() {
        let content = "---\ntitle: T\n---\nbody word one two\n";
        let chunks = chunk_document(content, &WordTokenizer, ChunkOptions::default());
        assert_eq!(chunks.len(), 1);
        assert!(
            !chunks[0].content.contains("title"),
            "frontmatter не в чанке"
        );
        assert!(chunks[0].content.starts_with("body"));
        // char_start указывает в тело исходного файла (после frontmatter).
        assert_eq!(
            &content[chunks[0].char_start..chunks[0].char_end],
            chunks[0].content
        );
    }

    #[test]
    fn splits_by_headings_with_path() {
        let content = "# Alpha\n\nintro text\n\n## Beta\n\nnested text here\n";
        let chunks = chunk_document(content, &WordTokenizer, ChunkOptions::default());
        let paths: Vec<_> = chunks.iter().map(|c| c.heading_path.clone()).collect();
        assert!(paths.contains(&Some("Alpha".to_string())));
        assert!(paths.contains(&Some("Alpha > Beta".to_string())));
    }

    #[test]
    fn large_section_slides_with_overlap() {
        // 30 слов, окно 10 токенов, overlap 3 → несколько чанков с перекрытием.
        let words = (0..30)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let chunks = chunk_document(&words, &WordTokenizer, opts(10, 3));
        assert!(chunks.len() > 1, "длинная секция должна разбиться");
        for c in &chunks {
            assert!(
                c.token_count <= 10,
                "чанк не превышает max_tokens (overlap внутри окна)"
            );
        }
        // overlap: следующий чанк начинается раньше конца предыдущего (повтор хвоста).
        assert!(
            chunks[1].char_start < chunks[0].char_end,
            "должно быть перекрытие"
        );
    }

    #[test]
    fn fenced_code_not_split() {
        let content =
            "intro\n\n```\ncode line a\ncode line b\ncode line c\ncode line d\n```\n\nouter\n";
        // маленький лимит — но fenced-блок атомарен и не рвётся.
        let chunks = chunk_document(content, &WordTokenizer, opts(4, 1));
        let code_chunk = chunks
            .iter()
            .find(|c| c.content.contains("```"))
            .expect("есть чанк с кодом");
        assert!(code_chunk.content.contains("code line a"));
        assert!(
            code_chunk.content.contains("code line d"),
            "весь блок кода в одном чанке"
        );
    }
}

/**
 * Self-hosted шрифты дизайн-системы (local-first/offline — без Google Fonts в рантайме):
 * подключаются бандлом через @fontsource. UI — Onest, моно/мета — JetBrains Mono,
 * редакторская проза/акценты — Source Serif 4 (вкл. курсив). См. docs/dev/design.md.
 */
import '@fontsource-variable/onest/index.css';
import '@fontsource-variable/source-serif-4/index.css';
import '@fontsource-variable/source-serif-4/wght-italic.css';
import '@fontsource/jetbrains-mono/400.css';
import '@fontsource/jetbrains-mono/500.css';
import '@fontsource/jetbrains-mono/600.css';
// STIX Two Math для MathML (KaTeX) на Win/Linux — свой @font-face БЕЗ latin-unicode-range (см. файл).
import './math-font.css';

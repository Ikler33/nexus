import React from 'react';
import ReactDOM from 'react-dom/client';
import './fonts';
import './i18n/setup';
import './stores/theme'; // применяет data-theme до рендера (без вспышки)
import './stores/prefs'; // применяет --editor-max-width (читаемая ширина) до рендера
import { App } from './App';
import './styles.css';
import './motion.css'; // motion-слой дизайн-системы: пружинные easing'и + brand-thinking (DP-0)

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);

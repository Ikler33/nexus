import React from 'react';
import ReactDOM from 'react-dom/client';
import './fonts';
import './i18n/setup';
import './stores/theme'; // применяет data-theme до рендера (без вспышки)
import { App } from './App';
import './styles.css';

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);

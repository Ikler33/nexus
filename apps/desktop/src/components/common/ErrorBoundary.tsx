import { Component, type ErrorInfo, type ReactNode } from 'react';
import { AlertTriangle } from 'lucide-react';
import i18n from '../../i18n/setup';
import { logUi } from '../../lib/debug-log';
import styles from './ErrorBoundary.module.css';

/**
 * ErrorBoundary per-contribution (F-8): изолирует падение зарегистрированного вклада (main-вью /
 * секция настроек) — рендер-ошибка внутри `children` ловится, вместо белого экрана показывается
 * плашка «модуль X упал» + кнопка перезагрузки. Цель владельца: «ИИ правит модуль → app не падает».
 *
 * Класс-компонент — единственный способ поймать рендер-ошибку в React (getDerivedStateFromError /
 * componentDidCatch). i18n берём из синглтона (в классе нет хуков; плашка — редкий аварийный путь,
 * язык зафиксирован при старте). Счастливый путь рендерит `children` БЕЗ обёртки-узла (нулевой
 * DOM-след — e2e-якори вью/секций не смещаются).
 */
interface Props {
  /** Отображаемое имя вклада для плашки (уже локализованное). */
  label: string;
  children: ReactNode;
}

interface State {
  error: Error | null;
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    // Диагностика в бэкенд-журнал (не console.error — тот ловит e2e-гейт). Только метаданные (AC-SEC-6).
    logUi(
      'module-error',
      `${this.props.label}: ${error.message}\n${info.componentStack ?? ''}`.slice(0, 400),
    );
  }

  render(): ReactNode {
    if (this.state.error) {
      return (
        <div className={styles.plate} role="alert">
          <AlertTriangle className={styles.icon} size={28} aria-hidden />
          <p className={styles.title}>{i18n.t('connector.moduleCrashed', { name: this.props.label })}</p>
          <p className={styles.sub}>{i18n.t('connector.moduleCrashedHint')}</p>
          <button
            type="button"
            className={styles.reload}
            onClick={() => window.location.reload()}
          >
            {i18n.t('connector.reload')}
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}

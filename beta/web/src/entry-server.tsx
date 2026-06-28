import { renderToString } from 'solid-js/web';
import { App } from './App';

export function renderShell() {
  return renderToString(() => <App />);
}

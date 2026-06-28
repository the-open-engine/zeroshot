import { hydrate } from 'solid-js/web';
import { App } from './App';
import './styles.css';

hydrate(() => <App />, document.getElementById('root') as HTMLElement);

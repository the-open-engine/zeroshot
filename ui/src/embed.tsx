import { createRoot } from 'react-dom/client';
import '@fontsource-variable/fraunces';
import '@fontsource-variable/spline-sans';
import '@xyflow/react/dist/style.css';
import './styles.css';
import { EmbeddedApp } from './EmbeddedApp';

createRoot(document.getElementById('root')!).render(<EmbeddedApp />);

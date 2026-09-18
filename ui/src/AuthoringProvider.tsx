import { createContext, useContext } from 'react';
import type { AuthoringService } from './workspace-services';

const unavailable = async (): Promise<never> => {
  throw new Error('Authoring is unavailable.');
};
const context = createContext<AuthoringService>({
  validate: unavailable,
  outcome: unavailable,
  data: unavailable,
});
export const AuthoringProvider = context.Provider;
export const useAuthoring = () => useContext(context);

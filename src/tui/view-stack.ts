export type ViewId = "launcher" | "monitor" | "cluster" | "agent";

export const DEFAULT_VIEW: ViewId = "launcher";

export function createViewStack(initialView: ViewId = DEFAULT_VIEW): ViewId[] {
  return [initialView];
}

export function activeView(stack: ViewId[]): ViewId {
  return stack[stack.length - 1] ?? DEFAULT_VIEW;
}

export function pushView(stack: ViewId[], view: ViewId): ViewId[] {
  return [...stack, view];
}

export function popView(stack: ViewId[]): ViewId[] {
  if (stack.length <= 1) {
    return stack.length === 0 ? [DEFAULT_VIEW] : stack;
  }
  return stack.slice(0, -1);
}

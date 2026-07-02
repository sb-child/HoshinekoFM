/* eslint-disable @typescript-eslint/no-explicit-any */
import zhCN from './zh-CN';

type LocaleFn = (...args: any[]) => string;

export function t(key: keyof typeof zhCN, ...args: any[]): string {
  const entry = zhCN[key] as string | LocaleFn;
  return typeof entry === 'function' ? entry(...args) : entry;
}

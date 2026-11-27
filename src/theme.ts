import { invoke } from '@tauri-apps/api/core';

export interface ThemeConfig {
  preset: string;
  accent: string;
  background: string;
  foreground: string;
  font: 'system' | 'serif' | 'mono';
}

export const DEFAULT_THEME: ThemeConfig = {
  preset: '玻璃拟态', accent: '#3D7EA6', background: '#F8F9FB', foreground: '#16181C', font: 'system',
};

export const THEME_PRESETS: ThemeConfig[] = [
  DEFAULT_THEME,
  { preset: '石墨夜色', accent: '#9CC9E2', background: '#0A1320', foreground: '#EEF4FA', font: 'system' },
  { preset: 'ChatGPT', accent: '#3566F0', background: '#FFFFFF', foreground: '#1A1C1F', font: 'system' },
  { preset: '暖纸', accent: '#9B5421', background: '#FBF6EB', foreground: '#352B23', font: 'serif' },
];

const fontFamilies = {
  system: '-apple-system, BlinkMacSystemFont, "PingFang SC", "HarmonyOS Sans SC", "Segoe UI", "Microsoft YaHei", sans-serif',
  serif: 'Georgia, "Songti SC", "SimSun", serif',
  mono: 'ui-monospace, Menlo, Consolas, "PingFang SC", monospace',
};

function mix(left: string, right: string, amount: number) {
  return '#' + [1, 3, 5].map(offset => {
    const a = parseInt(left.slice(offset, offset + 2), 16);
    const b = parseInt(right.slice(offset, offset + 2), 16);
    return Math.round(a + (b - a) * amount).toString(16).padStart(2, '0');
  }).join('');
}

function luminance(color: string) {
  const [red, green, blue] = [1, 3, 5].map(offset => {
    const channel = parseInt(color.slice(offset, offset + 2), 16) / 255;
    return channel <= .04045 ? channel / 12.92 : ((channel + .055) / 1.055) ** 2.4;
  });
  return red * .2126 + green * .7152 + blue * .0722;
}

function contrast(left: string, right: string) {
  const a = luminance(left);
  const b = luminance(right);
  return (Math.max(a, b) + .05) / (Math.min(a, b) + .05);
}

function legible(color: string, background: string, minimum: number) {
  const towards = contrast(background, '#000000') > contrast(background, '#FFFFFF') ? '#000000' : '#FFFFFF';
  let value = color;
  for (let step = 1; contrast(value, background) < minimum && step <= 10; step++) value = mix(color, towards, step / 10);
  return value;
}

export function themeTokens(theme: ThemeConfig) {
  const { accent, background: bg, foreground: fg, font } = theme;
  const onAccent = contrast(accent, '#FFFFFF') >= contrast(accent, '#000000') ? '#FFFFFF' : '#000000';
  return {
    bg, fg, background: bg, foreground: fg, accent, font: fontFamilies[font],
    surface: mix(bg, fg, .04), side: mix(bg, fg, .025),
    line: mix(bg, fg, .14), muted: mix(bg, fg, .78), faint: mix(bg, fg, .69),
    soft: mix(bg, accent, .10), hover: mix(bg, fg, .06),
    'accent-line': mix(bg, accent, .38), 'accent-text': legible(accent, bg, 5.5),
    'on-accent': onAccent,
    'accent-hover': mix(accent, onAccent === '#FFFFFF' ? '#000000' : '#FFFFFF', .12), 'graph-bg': mix(bg, accent, .025),
    'graph-line': mix(bg, fg, .28), 'graph-muted': mix(bg, fg, .43),
    'repo-a': legible(accent, bg, 3),
    'repo-b': legible('#318899', bg, 3),
    'repo-c': legible('#7564BE', bg, 3),
    danger: legible('#B83C30', bg, 5.5), 'danger-soft': mix(bg, '#B83C30', .08), 'danger-line': mix(bg, '#B83C30', .30),
    warning: legible('#966715', bg, 5.5), 'warning-soft': mix(bg, '#B48020', .09), 'warning-line': mix(bg, '#B48020', .32),
    success: legible('#2B7557', bg, 5.5),
  };
}

export function applyTheme(theme: ThemeConfig) {
  const root = document.documentElement;
  for (const [name, value] of Object.entries(themeTokens(theme))) root.style.setProperty('--' + name, value);
  root.dataset.theme = theme.preset;
  root.dataset.colorScheme = luminance(theme.background) < .2 ? 'dark' : 'light';
  window.dispatchEvent(new Event('aimemory-theme'));
}

export function themeGet(): Promise<ThemeConfig> {
  return invoke('theme_get');
}

export function themeSave(theme: ThemeConfig): Promise<ThemeConfig> {
  return invoke('theme_save', { theme });
}

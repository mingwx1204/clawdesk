import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter } from 'react-router'
import { Toaster } from 'sonner'
import { playClick, initSound } from '@/lib/sound'
import './index.css'
import App from './App.tsx'

// AudioContext 需首次用户交互后激活
let soundReady = false;
document.addEventListener('click', (e) => {
  if (!soundReady) { initSound(); soundReady = true; }
  const el = e.target as HTMLElement;
  if (el.closest('button, [role="button"], [role="tab"], [role="menuitem"]')) {
    // 仅在设置中开启音效时播放
    import('@/store/useSettingsStore').then(({ useSettingsStore }) => {
      if (useSettingsStore.getState().settings.soundEnabled) playClick();
    });
  }
});

// 全局：防止 sonner 捕获空错误显示为 "undefined" toast
const origError = window.onerror;
window.onerror = (...args: unknown[]) => {
  if (typeof args[0] === 'string' && (args[0] === 'undefined' || !(args[0] as string).trim())) return true;
  return origError ? origError.apply(window, args as never) : false;
};
window.addEventListener('unhandledrejection', (e) => {
  if (!e.reason || String(e.reason) === 'undefined') e.preventDefault();
});

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <BrowserRouter>
      <App />
      <Toaster
        position="top-right"
        richColors
        closeButton
        toastOptions={{
          // 防止 undefined/null 消息显示
          duration: 3000,
        }}
      />
    </BrowserRouter>
  </StrictMode>,
)

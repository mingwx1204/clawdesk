/**
 * TTS 语音朗读 — 支持暂停/恢复/停止
 * 
 * 桌面端（Tauri）：调用 Rust 侧 Windows SAPI，每次朗读都跟随系统当前默认播放设备，
 *   解决 WebView2 speechSynthesis 缓存音频设备导致"切耳机无效"的问题。
 * 浏览器模式：使用 Web Speech API。
 */

let currentUtterance: SpeechSynthesisUtterance | null = null;
let isSpeaking = false;
let voicesLoaded = false;

/** 检查 TTS 是否可用 */
export function isTtsAvailable(): boolean {
  if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
    // 桌面端：依赖 Rust SAPI
    return true;
  }
  return typeof window !== 'undefined' && 'speechSynthesis' in window;
}

/** 确保语音列表已加载（浏览器模式专用，Chrome 需要异步） */
async function ensureVoices(): Promise<SpeechSynthesisVoice[]> {
  if (voicesLoaded) return speechSynthesis.getVoices();
  const voices = speechSynthesis.getVoices();
  if (voices.length > 0) { voicesLoaded = true; return voices; }
  // Chrome 异步加载 voices
  return new Promise((resolve) => {
    speechSynthesis.onvoiceschanged = () => {
      voicesLoaded = true;
      resolve(speechSynthesis.getVoices());
    };
  });
}

/** 音色信息 */
export interface TtsVoice {
  name: string;
  lang: string;
}

/** 枚举系统可用音色（桌面端走 Rust SAPI；浏览器用 Web Speech API） */
export async function listVoices(): Promise<TtsVoice[]> {
  if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
    const { invoke } = await import('@tauri-apps/api/core');
    try {
      return await invoke<TtsVoice[]>('tts_list_voices');
    } catch {
      return [];
    }
  }
  if (!isTtsAvailable()) return [];
  const voices = await ensureVoices();
  return voices.map((v) => ({ name: v.name, lang: v.lang }));
}

/** 切换音色（桌面端 Rust SAPI；浏览器端仅记录，speak 时应用） */
export async function setVoice(name: string): Promise<void> {
  if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
    const { invoke } = await import('@tauri-apps/api/core');
    try {
      await invoke('tts_set_voice', { voice: name });
    } catch { /* 静默 */ }
    return;
  }
  selectedVoice = name;
}

/** 当前选中的音色（浏览器模式用） */
let selectedVoice = '';

/** 朗读文本（桌面端用 Rust SAPI，浏览器用 Web Speech API） */
export async function speak(text: string, onStart?: () => void, onEnd?: () => void): Promise<void> {
  if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
    const { invoke } = await import('@tauri-apps/api/core');
    onStart?.();
    try {
      await invoke('tts_speak', { text });
      // 等待朗读完成（通过事件或延迟触发 onEnd）
      await new Promise((r) => setTimeout(r, Math.min(text.length * 80, 60000)));
      onEnd?.();
    } catch {
      onEnd?.();
    }
    return;
  }

  if (!isTtsAvailable()) return;
  stopSpeaking();

  // Web Speech API 有字符限制，但实测中文可到 3000+
  const maxLen = 4000;
  const content = text.length > maxLen ? text.slice(0, maxLen) + '…后续内容请查看屏幕' : text;

  const utterance = new SpeechSynthesisUtterance(content);
  utterance.rate = 1.25;   // 稍快
  utterance.pitch = 1.0;
  utterance.volume = 0.9;

  // 选择音色：优先用户选中的，否则自动选中文语音
  const voices = await ensureVoices();
  let voice = selectedVoice
    ? voices.find(v => v.name === selectedVoice)
    : undefined;
  if (!voice) {
    voice = voices.find(v => v.lang.startsWith('zh-CN'))
      ?? voices.find(v => v.lang.startsWith('zh'));
  }
  if (voice) utterance.voice = voice;

  utterance.onstart = () => { isSpeaking = true; onStart?.(); };
  utterance.onend = () => { if (currentUtterance === utterance) { isSpeaking = false; onEnd?.(); } };
  utterance.onerror = (e) => { console.warn('[TTS] 朗读出错:', e); if (currentUtterance === utterance) { isSpeaking = false; onEnd?.(); } };

  currentUtterance = utterance;
  speechSynthesis.speak(utterance);
}

/** 停止朗读 */
export function stopSpeaking(): void {
  // 桌面端：通知 Rust SAPI 停止
  if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
    void import('@tauri-apps/api/core').then(({ invoke }) => {
      void invoke('tts_stop').catch(() => {});
    });
    isSpeaking = false;
    currentUtterance = null;
    return;
  }
  if (isSpeaking || speechSynthesis.speaking) {
    speechSynthesis.cancel();
    isSpeaking = false;
    currentUtterance = null;
  }
}

/** 当前是否正在朗读 */
export function getIsSpeaking(): boolean {
  if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
    return isSpeaking;
  }
  return isSpeaking || speechSynthesis.speaking;
}

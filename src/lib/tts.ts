/**
 * ClawDesk 朗读引擎 —— Edge TTS（微软神经网络拟人语音）。
 *
 * 通过 Rust 后端 `tts_speak` 命令合成 MP3（后端 WebSocket 带浏览器指纹直连微软服务，
 * 浏览器 WebSocket 无法设置自定义 headers，故必须走后端），前端用 Audio 播放。
 *
 * 相比系统 Web Speech API：
 * - 音色更多更自然（晓晓/云希/云健…全部 Neural 神经网络语音）
 * - 支持语气风格（开心/温柔/平静/严肃…）
 * - 语速可调 0.5~2.0
 */
import { invoke } from "@tauri-apps/api/core";

/** 音色信息（与后端 TtsVoiceInfo 镜像）。 */
export interface TtsVoice {
  id: string;
  name: string;
  gender: string;
  desc: string;
  region: string;
  styles: string[];
}

/** 合成结果（与后端 TtsResult 镜像）。 */
interface TtsResult {
  audioBase64: string;
  bytes: number;
  voice: string;
}

/** 朗读设置（与后端 AppSettings tts* 字段镜像）。 */
export interface TtsSettings {
  enabled: boolean;
  voice: string;
  rate: number;
  style: string;
}

let currentAudio: HTMLAudioElement | null = null;
let speaking = false;
let lastSettings: TtsSettings | null = null;

/** 读取当前朗读设置（后端持久化）。 */
export async function loadTtsSettings(): Promise<TtsSettings> {
  try {
    const s = await invoke<any>("settings_get");
    lastSettings = {
      enabled: s?.ttsEnabled !== false,
      voice: s?.ttsVoice || "zh-CN-XiaoxiaoNeural",
      rate: typeof s?.ttsRate === "number" ? s.ttsRate : 1.0,
      style: s?.ttsStyle || "",
    };
  } catch {
    lastSettings = { enabled: true, voice: "zh-CN-XiaoxiaoNeural", rate: 1.0, style: "" };
  }
  return lastSettings;
}

/** 当前朗读设置（已加载时直接返回，否则异步加载）。 */
export async function getTtsSettings(): Promise<TtsSettings> {
  if (lastSettings) return lastSettings;
  return loadTtsSettings();
}

/** 列出内置音色（设置界面下拉数据源）。 */
export async function listVoices(): Promise<TtsVoice[]> {
  try {
    return await invoke<TtsVoice[]>("tts_list_voices");
  } catch {
    return [];
  }
}

/** 是否正在朗读。 */
export function getIsSpeaking(): boolean {
  return speaking;
}

/** 停止当前朗读。 */
export function stopSpeaking(): void {
  if (currentAudio) {
    try { currentAudio.pause(); } catch { /* 忽略 */ }
    currentAudio = null;
  }
  speaking = false;
}

/**
 * 朗读指定文本（Edge TTS 合成 → 播放）。
 * @param text  要朗读的文本
 * @param voice 音色 ID（默认晓晓）
 * @param rate  语速 0.5~2.0（默认 1.0）
 * @param style 语气风格（默认自然）
 * @returns 成功 true / 失败 false
 */
export async function speak(
  text: string,
  voice?: string,
  rate?: number,
  style?: string,
): Promise<boolean> {
  stopSpeaking();
  const clean = (text || "")
    .replace(/[#*`>\[\]\-~|_]/g, " ")
    .replace(/\s+/g, " ")
    .trim();
  if (!clean) return false;

  const s = await getTtsSettings();
  const v = voice || s.voice || "zh-CN-XiaoxiaoNeural";
  const r = rate ?? s.rate ?? 1.0;
  const st = style ?? s.style ?? "";

  try {
    const res = await invoke<TtsResult>("tts_speak", {
      text: clean,
      voice: v,
      rate: r,
      style: st,
    });
    const audio = new Audio("data:audio/mpeg;base64," + res.audioBase64);
    currentAudio = audio;
    speaking = true;
    audio.onended = () => {
      speaking = false;
      currentAudio = null;
    };
    audio.onerror = () => {
      speaking = false;
      currentAudio = null;
    };
    await audio.play();
    return true;
  } catch (e) {
    console.error("[TTS] 合成/播放失败:", e);
    speaking = false;
    currentAudio = null;
    return false;
  }
}

/** 试听音色（设置界面用，固定短文本）。 */
export async function previewVoice(voice: string, rate: number, style: string): Promise<boolean> {
  return speak(
    "你好呀！我是你的朗读助手，听听这个音色怎么样？如果你喜欢，就在设置里选我吧。",
    voice,
    rate,
    style,
  );
}

/** 语气风格中文标签。 */
export function styleLabel(style: string): string {
  const map: Record<string, string> = {
    cheerful: "😄 开心",
    empathetic: "💗 温柔共情",
    calm: "😌 平静",
    gentle: "🌸 温和",
    serious: "📌 严肃",
    newscast: "📰 新闻播报",
    sad: "😢 悲伤",
    angry: "😠 生气",
    excited: "🎉 兴奋",
    fearful: "😨 害怕",
    lyrical: "🎵 抒情",
    "poetry-reading": "📖 诗歌朗诵",
  };
  return map[style] || "🌿 自然（无语气）";
}

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/**
 * 微信自动回复逻辑（从 App.vue 下沉的独立 composable）。
 *
 * 职责：监听后端 `wechat-message` 事件 → 组装 prompt（时间感知 + 真人聊天风格
 * + 媒体清单 + 引用上下文）→ 调 agent_chat → 回发文本/生图。
 *
 * 设计：通过 `getApiKey` 依赖注入 API Key（避免与组件响应式状态耦合），
 * 其余能力（invoke/listen/localStorage）均为独立导入，组件只负责挂载。
 */

/** 微信自动回复器：构造一次，挂载监听，可随时按开关/Key 跳过 */
export function useWechatAutoReply(getApiKey: () => string) {
  // ★ 每槽位并发锁：同一微信的多条消息串行回复，防止并发 agent_chat
  //   对同一会话（wechat-{slot}）读-改-写竞争导致记忆丢失/回复乱序
  const slotLocks = new Map<number, Promise<unknown>>();

  function withSlotLock(slot: number, fn: () => Promise<void>): Promise<void> {
    const prev = slotLocks.get(slot) ?? Promise.resolve();
    const next = prev.then(fn, fn);
    slotLocks.set(slot, next);
    next.finally(() => {
      if (slotLocks.get(slot) === next) slotLocks.delete(slot);
    }).catch(() => {});
    return next;
  }

  /**
   * 收到微信用户消息 → AI 自动回复（每槽位独立会话记忆 + 独立人设）。
   * msg 来自后端 wechat-message 事件（WechatMessage）：
   *   content      文本（含语音云端转写 `[语音] …`、引用消息注记）
   *   images       图片本地路径（AI 用 analyze_image 读取）
   *   attachments  文件/语音/视频本地路径（AI 用 file_read 读取）
   *   botSlot      所属微信槽位（0 = 微信1 …）
   */
  async function autoReplyWechat(msg: any) {
    if (!msg || !msg.fromUser) return;
    const hasMedia =
      (Array.isArray(msg.images) && msg.images.length) ||
      (Array.isArray(msg.attachments) && msg.attachments.length);
    if (!msg.content && !hasMedia) return; // 纯媒体消息也能回复（AI 看图/读文件）
    // 开关：ClawDesk 微信面板（WechatPanel）可切换，默认开启
    if (localStorage.getItem("clawdesk_wechat_autoreply") === "off") return;
    const apiKey = getApiKey().trim();
    if (!apiKey) return;
    // ★ 所属微信槽位（0 = 微信1 …）：每个微信独立 AI 会话记忆 + 独立人设
    const slot = typeof msg.botSlot === "number" ? msg.botSlot : 0;
    // 读取该微信的人设（后端 wechat 槽位 persona，已随账号恢复）
    let persona: string | null = null;
    try {
      const st = await invoke<any>("wechat_bot_status");
      const bots = st?.bots || [];
      const b = bots.find((x: any) => x.slot === slot);
      if (b?.personaText) persona = b.personaText;
    } catch { /* 读取失败忽略 */ }
    try {
      // ★ 时间感知：把当前时间告诉 AI，让它根据时间决定说话方式（如深夜说"这么晚找我什么事"）
      const now = new Date();
      const wd = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"][now.getDay()];
      const h = now.getHours();
      const part =
        h < 5 ? "凌晨" : h < 8 ? "清晨" : h < 12 ? "上午" : h < 14 ? "中午" : h < 18 ? "下午" : h < 23 ? "晚上" : "深夜";
      const timeNote = `\n\n[当前时间：${wd}，${part} ${h}点${String(now.getMinutes()).padStart(2, "0")}分。请结合当前时间说话：深夜/凌晨回复要带"这么晚找我"的关心感，清晨问早，白天正常聊。]`;
      // ★ 生活状态注入（世界线 · 一生记忆）：AI 此刻在做什么 + 今日轨迹 +
      //   近期记忆 + 出生日期。用户问"你干嘛呢/你昨天干嘛了"能连贯回答。
      //   ★ 人设兼容：生活状态是"世界日常节奏"背景而非硬性规定——AI 会结合
      //   人设自然演绎（猫→晒太阳、机器人→待机充电），不与人设冲突。
      let livingNote = "";
      try {
        const living = await invoke<string>("wechat_living_context");
        if (living) livingNote = `\n\n[你的世界日常节奏（这是你所在世界的真实时间线，时间与真实时钟同步。但它只是背景参考，你必须结合自己的人设自然演绎你的生活，不必照搬人类活动——比如你是猫就演绎成晒太阳/追毛线，你是机器人就演绎成待机/充电，你是修仙者就演绎成闭关/炼丹。用户问你在干嘛/你最近在干嘛时按此回答）：${living}]`;
      } catch { /* 状态获取失败忽略 */ }
      // ★ 微信真人聊天风格约束（去 AI 味）：微信聊天要像真人朋友发消息，
      //   不是写文章——短、口语、有情绪、不解释过程。
      const styleNote = `\n\n[微信聊天铁律（必须严格遵守）：\n1. 默认回复 5~40 字，一句话说清，绝不超过 60 字；\n2. 除非用户明确要求（"写500字"/"详细说说"/"完整分析"等），否则一律像真人发微信：口语化、短句、可省略主语、偶尔语气词（嗯嗯/哈哈哈/行/好嘞）；\n3. 禁止 AI 腔：不用"首先/其次/总之/需要注意的是/总的来说"，不用"！"堆砌，不用"哦～""呢～"等做作语气；\n4. 不需要解释你怎么做到的、不需要总结性发言、不要每句都带 emoji（最多 1 个）；\n5. 对方问了复杂问题（如读文件/分析代码）时也只需给结论和关键点，别列清单；\n6. 像朋友一样接话，而不是像客服回答问题。]`;
      // 微信发来的媒体（图片/文件/语音/视频）已由后端下载解密到本地，拼入 prompt 让 AI 读取
      let promptText = (msg.content || "") + timeNote + livingNote + styleNote;
      const mediaNotes: string[] = [];
      if (Array.isArray(msg.images) && msg.images.length) {
        mediaNotes.push("图片：\n" + msg.images.map((p: string) => `- ${p}`).join("\n"));
      }
      if (Array.isArray(msg.attachments) && msg.attachments.length) {
        mediaNotes.push("文件/语音/视频：\n" + msg.attachments.map((p: string) => `- ${p}`).join("\n"));
      }
      if (mediaNotes.length) {
        promptText +=
          "\n\n[用户微信发来的媒体，已保存到本地磁盘]\n" +
          mediaNotes.join("\n") +
          "\n请调用 analyze_image 工具读取图片内容、file_read 工具读取文件内容（zip 压缩包可直接用 file_read 读取，超过 2MB 的压缩包不支持并如实告知）。";
      }
      // ★ AI 生成期间显示"对方正在输入"（后端保活，回复后 finally 关闭）
      startWechatTyping(msg.fromUser, slot);
      const outcome = await invoke<any>("agent_chat", {
        apiKey,
        sessionId: `wechat-${slot}`, // ★ 每个微信独立会话记忆（wechat-0 / wechat-1 …）
        runId: `wechat-${Date.now()}-${Math.floor(Math.random() * 1000)}`,
        prompt: promptText,
        resume: true,
        persona, // ★ 该微信的人设（system prompt 注入）
      });
      const reply = (outcome?.finalText || "").trim();
      // ★ AI 回复时若调用过 generate_image 生图，把生成的图片路径一并发给微信
      const generatedImages: string[] = [];
      const rounds: any[] = Array.isArray(outcome?.rounds) ? outcome.rounds : [];
      for (const r of rounds) {
        for (const tc of Array.isArray(r.toolCalls) ? r.toolCalls : []) {
          if (tc.toolId === "generate_image" && tc.status === "success" && tc.output?.path) {
            const p = String(tc.output.path);
            if (!generatedImages.includes(p)) generatedImages.push(p);
          }
        }
      }
      if (generatedImages.length) {
        for (const imgPath of generatedImages) {
          try {
            await invoke("wechat_send_image", { toUser: msg.fromUser, imagePath: imgPath, slot });
            console.log(`[wechat] 已发送图片到 ${msg.fromUser}: ${imgPath}`);
          } catch (e) {
            console.error("[wechat] 发送图片失败", e);
          }
        }
      }
      if (!reply) return;
      await invoke("wechat_bot_reply", {
        msgId: msg.msgId,
        toUser: msg.fromUser,
        content: reply,
        slot,
      });
      console.log(`[wechat] 微信${slot + 1} 已自动回复 ${msg.fromUser}: ${reply.slice(0, 60)}`);
      // ★ AI 语音回复（可选，默认关）：开关开启时，AI 回复文本后再发一条
      //   Edge TTS 真人音色合成的语音文件（微信 iLink 官方协议不支持语音条，
      //   发文件是官方协议下的最佳近似——对方点开即可播放真人语音）。
      if (localStorage.getItem("clawdesk_wechat_voicereply") === "on") {
        // 不传 voice：后端按已保存的 voice_id 合成（Edge/CosyVoice 共用同一字段），
        // 避免旧代码硬编码 Edge 默认音色覆盖用户选择的 CosyVoice 音色
        try {
          await invoke("wechat_send_voice", { toUser: msg.fromUser, text: reply, slot });
          console.log(`[wechat] 语音回复已发送 ${msg.fromUser}`);
        } catch (e) {
          console.error("[wechat] 语音回复发送失败", e);
        }
      }
    } catch (e) {
      console.error("微信自动回复失败", e);
    } finally {
      // ★ 停止"正在输入"保活（无论成功/失败都结束输入态，避免对方一直看到输入中）
      void invoke("wechat_typing", { toUser: msg.fromUser, active: false, slot }).catch(() => {});
    }
  }

  /**
   * AI 生成期间向对方显示"对方正在输入"：
   * 后端 wechat_typing 启动保活任务（每 10s 发一次 typing），agent_chat 结束后由
   * autoReplyWechat 的 finally 关闭。生成前调用一次即可（保活由后端维持）。
   */
  function startWechatTyping(toUser: string, slot: number) {
    void invoke("wechat_typing", { toUser, active: true, slot }).catch(() => {});
  }

  /** 挂载微信消息监听（组件 onMounted 时调用，返回解除函数） */
  function listenWechatMessages(): Promise<UnlistenFn> {
    return listen<any>("wechat-message", (e) => {
      const slot = typeof e.payload?.botSlot === "number" ? e.payload.botSlot : 0;
      void withSlotLock(slot, () => autoReplyWechat(e.payload));
    });
  }

  return { autoReplyWechat, listenWechatMessages, startWechatTyping };
}

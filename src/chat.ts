// FlashTrans AI 对话（F4）
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  loadSettings, activeProfile, applyTheme, applyScale, onSettingsChanged,
  llmStream, el, ICONS, mountWinControls, type Settings,
} from "./shared";

const win = getCurrentWindow();
const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;

let settings: Settings;
let history: { role: string; content: string }[] = [];
let context: { source: string; translated: string } | null = null;
let streaming = false;

function refreshBadge() {
  const b = $("model-badge");
  const p = activeProfile(settings);
  b.textContent = p.model || "未配置 API";
  b.className = p.model ? "badge" : "badge blue";
}

function scrollBottom() {
  const m = $("msgs");
  m.scrollTop = m.scrollHeight;
}

function addMsg(role: "user" | "ai", text: string, quote?: string): HTMLElement {
  $("empty").style.display = "none";
  const wrap = el("div", `msg ${role}`);
  if (quote) {
    wrap.appendChild(el("div", "quote", quote));
  }
  const bubble = el("div", "bubble", text);
  wrap.appendChild(bubble);
  $("msgs").appendChild(wrap);
  scrollBottom();
  return bubble;
}

function setContext(source: string, translated: string) {
  context = { source, translated };
  const brief = source.length > 60 ? source.slice(0, 60) + "…" : source;
  $("ctx-text").textContent = brief;
  $("ctx-chip").style.display = "";
}

function clearContext() {
  context = null;
  $("ctx-chip").style.display = "none";
}

async function send() {
  if (streaming) return;
  const input = $<HTMLTextAreaElement>("input");
  const q = input.value.trim();
  if (!q) return;

  const p = activeProfile(settings);
  if (!p.baseUrl || !p.model) {
    addMsg("ai", "尚未配置在线 API。请按 F5 打开主窗口，在设置中填写 OpenAI 兼容 API。").classList.add("err");
    return;
  }

  input.value = "";
  input.style.height = "auto";

  const quote = context
    ? `${context.source}${context.translated ? " ⇢ " + context.translated : ""}`
    : undefined;
  addMsg("user", q, quote && (quote.length > 80 ? quote.slice(0, 80) + "…" : quote));

  let userContent = q;
  if (context) {
    userContent =
      `[引用内容]\n原文：${context.source}\n` +
      (context.translated ? `译文：${context.translated}\n` : "") +
      `\n[问题]\n${q}`;
    clearContext();
  }
  history.push({ role: "user", content: userContent });

  const messages = [
    {
      role: "system",
      content: "You are FlashTrans AI, a helpful assistant. Answer in Chinese unless the user asks otherwise. Be concise and clear. /no_think",
    },
    ...history.slice(-20),
  ];

  streaming = true;
  $<HTMLButtonElement>("btn-send").disabled = true;
  const bubble = addMsg("ai", "");
  const caret = el("span", "stream-caret");
  bubble.appendChild(caret);

  await llmStream(p, messages, {
    onDelta: (t) => {
      caret.insertAdjacentText("beforebegin", t);
      scrollBottom();
    },
    onDone: (full) => {
      bubble.textContent = full;
      history.push({ role: "assistant", content: full });
      streaming = false;
      $<HTMLButtonElement>("btn-send").disabled = false;
      scrollBottom();
      $("input").focus();
    },
    onError: (m) => {
      bubble.textContent = m;
      bubble.classList.add("err");
      streaming = false;
      $<HTMLButtonElement>("btn-send").disabled = false;
    },
  }, 0.4);
}

async function main() {
  settings = await loadSettings();
  applyTheme(settings.theme);
  applyScale(settings.uiScale);
  refreshBadge();
  onSettingsChanged((s) => {
    settings = s;
    applyTheme(s.theme);
    applyScale(s.uiScale);
    refreshBadge();
  });

  mountWinControls();
  $("btn-send").innerHTML = ICONS.send;
  $("btn-clear").innerHTML = ICONS.trash;

  $("tb-close").addEventListener("click", () => win.hide());
  $("tb-min").addEventListener("click", () => win.minimize());
  $("tb-max").addEventListener("click", () => win.toggleMaximize());

  $("btn-clear").addEventListener("click", () => {
    history = [];
    $("msgs").querySelectorAll(".msg").forEach((n) => n.remove());
    $("empty").style.display = "";
    clearContext();
  });

  $("ctx-x").addEventListener("click", clearContext);
  $("btn-send").addEventListener("click", send);

  const input = $<HTMLTextAreaElement>("input");
  input.addEventListener("input", () => {
    input.style.height = "auto";
    input.style.height = Math.min(140, input.scrollHeight) + "px";
  });
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      send();
    } else if (e.key === "Escape") {
      win.hide();
    }
  });

  await listen("chat-open", (e) => {
    const p = e.payload as { clipboard?: string; context?: { source: string; translated: string } };
    if (p.context && p.context.source) {
      setContext(p.context.source, p.context.translated ?? "");
    } else if (p.clipboard) {
      setContext(p.clipboard, "");
    }
    setTimeout(() => $("input").focus(), 80);
  });
}

window.addEventListener("DOMContentLoaded", main);

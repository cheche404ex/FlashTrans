import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import {
  activeDomainPrompt, activeProfile, applyScale, applyTheme, comboFromEvent, el,
  ensurePrompts, ICONS, LANGS_FULL, llmStream, loadSettings, makeDropdown,
  makeSearchDropdown, mountWinControls, onSettingsChanged, resolveTarget,
  saveSettings, toast, translateMessages, type ApiProfile, type SearchItem, type Settings,
} from "./shared";

const HK_DEFAULTS: Record<string, string> = { f1: "F1", f2: "F2", f3: "F3", f4: "F4", f5: "F5" };
const LANG_ITEMS: SearchItem[] = LANGS_FULL.map(([code, zh, en]) => ({
  value: code, label: zh, sub: en, search: `${zh} ${en} ${code}`.toLowerCase(),
}));
const win = getCurrentWindow();
const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;

let settings: Settings;
let translating = false;
let selectionEnabled = false;
let recordingHotkey: { key: string; button: HTMLButtonElement } | null = null;

function segSet(id: string, value: string) {
  document.querySelectorAll(`#${id} button`).forEach((button) => {
    const selected = (button as HTMLElement).dataset.v === value;
    button.classList.toggle("on", selected);
    button.setAttribute("aria-pressed", String(selected));
  });
}

function setRequestState(text: string, busy = false) {
  const state = $("request-state");
  state.textContent = text;
  state.classList.toggle("busy", busy);
}

function refreshApiBadge() {
  const badge = $("backend-badge");
  const profile = activeProfile(settings);
  if (profile.baseUrl && profile.model) {
    badge.textContent = profile.model;
    badge.className = "badge";
  } else {
    badge.textContent = "未配置 API";
    badge.className = "badge blue";
  }
}

function refreshSelectionState(enabled: boolean) {
  selectionEnabled = enabled;
  $("selection-toggle").classList.toggle("on", enabled);
  $("selection-toggle").setAttribute("aria-pressed", String(enabled));
  $("selection-label").textContent = enabled ? "划词已开启" : "划词已关闭";
}

function currentProfile(): ApiProfile {
  let profile = settings.api.profiles.find((item) => item.name === settings.api.selected);
  if (!profile) profile = settings.api.profiles[0];
  if (!profile) {
    profile = { name: "default", baseUrl: "", apiKey: "", model: "", supportsVision: false };
    settings.api.profiles.push(profile);
  }
  profile.supportsVision = Boolean(profile.supportsVision);
  settings.api.selected = profile.name;
  return profile;
}

function showResult(text: string, error = false) {
  const output = $("dst-text");
  output.textContent = text;
  output.classList.toggle("err", error);
  $("dst-empty").style.display = text ? "none" : "";
  $("dst-skeleton").style.display = "none";
}

function beginLoading() {
  $("dst-text").textContent = "";
  $("dst-text").classList.remove("err");
  $("dst-empty").style.display = "none";
  $("dst-skeleton").style.display = "";
  setRequestState("正在翻译", true);
}

async function translate() {
  if (translating) return;
  const text = $<HTMLTextAreaElement>("src-text").value.trim();
  if (!text) return;
  const profile = activeProfile(settings);
  if (!profile.baseUrl || !profile.model) {
    showResult("尚未配置在线 API。请打开设置填写 Base URL、Model 和 API Key。", true);
    return;
  }
  translating = true;
  $<HTMLButtonElement>("btn-translate").disabled = true;
  beginLoading();
  const target = resolveTarget(text, settings.sourceLang, settings.targetLang);
  let started = false;
  const output = $("dst-text");
  const caret = el("span", "stream-caret");
  await llmStream(profile, translateMessages(text, target, false, activeDomainPrompt(settings)), {
    onDelta: (part) => {
      if (!started) {
        started = true;
        $("dst-skeleton").style.display = "none";
        output.appendChild(caret);
      }
      caret.insertAdjacentText("beforebegin", part);
    },
    onDone: (full) => {
      showResult(full || "（无翻译结果）");
      translating = false;
      $<HTMLButtonElement>("btn-translate").disabled = false;
      setRequestState("完成");
    },
    onError: (message) => {
      showResult(message, true);
      translating = false;
      $<HTMLButtonElement>("btn-translate").disabled = false;
      setRequestState("请求失败");
    },
  });
}

async function main() {
  settings = await loadSettings();
  applyTheme(settings.theme);
  applyScale(settings.uiScale);
  mountWinControls();
  $("btn-chat").innerHTML = ICONS.chat;
  $("btn-settings").innerHTML = ICONS.gear;
  $("btn-swap").innerHTML = ICONS.swap;
  $("src-copy").innerHTML = ICONS.copy;
  $("src-clear").innerHTML = ICONS.clear;
  $("dst-copy").innerHTML = ICONS.copy;
  $("sheet-close").innerHTML = ICONS.clear;
  refreshApiBadge();

  refreshSelectionState(await invoke<boolean>("selection_mode_get"));
  await listen<boolean>("selection-mode-changed", (event) => refreshSelectionState(Boolean(event.payload)));

  $("selection-toggle").addEventListener("click", async () => {
    await invoke("selection_mode_set", { enabled: !selectionEnabled });
  });
  $("tb-close").addEventListener("click", () => win.hide());
  $("tb-min").addEventListener("click", () => win.minimize());
  $("tb-max").addEventListener("click", () => win.toggleMaximize());

  const sourceDropdown = makeSearchDropdown(LANG_ITEMS, settings.sourceLang, async (value) => {
    settings.sourceLang = value;
    await saveSettings(settings);
  });
  const targetDropdown = makeSearchDropdown(LANG_ITEMS, settings.targetLang, async (value) => {
    settings.targetLang = value;
    await saveSettings(settings);
  });
  $("dd-src").appendChild(sourceDropdown.root);
  $("dd-tgt").appendChild(targetDropdown.root);
  const swapButton = $("btn-swap");
  swapButton.addEventListener("animationend", () => swapButton.classList.remove("swapping"));
  swapButton.addEventListener("click", async () => {
    swapButton.classList.remove("swapping");
    void swapButton.offsetWidth;
    swapButton.classList.add("swapping");
    const source = settings.sourceLang === "auto" ? "en" : settings.sourceLang;
    const target = settings.targetLang === "auto" ? "zh" : settings.targetLang;
    settings.sourceLang = target;
    settings.targetLang = source;
    sourceDropdown.set(target);
    targetDropdown.set(source);
    await saveSettings(settings);
  });

  const sourceText = $<HTMLTextAreaElement>("src-text");
  sourceText.addEventListener("input", () => $("src-count").textContent = sourceText.value ? String(sourceText.value.length) : "");
  sourceText.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
      event.preventDefault();
      translate();
    }
  });
  $("src-copy").addEventListener("click", async () => {
    if (!sourceText.value) return;
    await invoke("copy_text", { text: sourceText.value });
    toast("已复制原文");
  });
  $("src-clear").addEventListener("click", () => {
    sourceText.value = "";
    $("src-count").textContent = "";
    showResult("");
    sourceText.focus();
  });
  $("dst-copy").addEventListener("click", async () => {
    const text = $("dst-text").textContent || "";
    if (!text) return;
    await invoke("copy_text", { text });
    toast("已复制译文");
  });
  $("btn-translate").addEventListener("click", translate);

  const sheet = $("sheet");
  const mask = $("sheet-mask");
  const app = $("app");
  const settingsButton = $<HTMLButtonElement>("btn-settings");
  let sheetCloseTimer: number | undefined;

  const closeSheet = () => {
    if (!sheet.classList.contains("on")) return;
    window.clearTimeout(sheetCloseTimer);
    sheet.classList.remove("on");
    sheet.classList.add("closing");
    sheet.setAttribute("aria-hidden", "true");
    mask.classList.remove("on");
    mask.setAttribute("aria-hidden", "true");
    settingsButton.setAttribute("aria-expanded", "false");
    app.inert = false;
    settingsButton.focus({ preventScroll: true });
    sheetCloseTimer = window.setTimeout(() => sheet.classList.remove("closing"), 190);
  };

  const openSheet = () => {
    window.clearTimeout(sheetCloseTimer);
    fillSettings();
    sheet.classList.remove("closing", "on");
    void sheet.offsetWidth;
    sheet.classList.add("on");
    sheet.setAttribute("aria-hidden", "false");
    mask.classList.add("on");
    mask.setAttribute("aria-hidden", "false");
    settingsButton.setAttribute("aria-expanded", "true");
    app.inert = true;
    requestAnimationFrame(() => $<HTMLButtonElement>("sheet-close").focus({ preventScroll: true }));
  };

  settingsButton.addEventListener("click", openSheet);
  $("sheet-close").addEventListener("click", closeSheet);
  mask.addEventListener("click", closeSheet);
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && !recordingHotkey) {
      closeSheet();
      return;
    }
    if (event.key !== "Tab" || !sheet.classList.contains("on")) return;
    const focusable = Array.from(sheet.querySelectorAll<HTMLElement>(
      'button:not([disabled]), input:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
    )).filter((element) => element.getClientRects().length > 0);
    if (!focusable.length) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  });

  function fillSettings() {
    renderProfiles();
    fillProfileEditor();
    renderPrompts();
    renderHotkeys();
    segSet("seg-ocr", settings.ocr.mode);
    segSet("seg-theme", settings.theme);
    segSet("seg-scale", String(settings.uiScale));
    segSet("seg-stay", String(settings.popupStaySecs));
    segSet("seg-clickout", settings.closeOnBlur ? "1" : "0");
    refreshOcrHint();
  }

  function fillProfileEditor() {
    const profile = currentProfile();
    $<HTMLInputElement>("api-name").value = profile.name;
    $<HTMLInputElement>("api-base").value = profile.baseUrl;
    $<HTMLInputElement>("api-model").value = profile.model;
    $<HTMLInputElement>("api-key").value = profile.apiKey;
    $<HTMLInputElement>("api-vision").checked = Boolean(profile.supportsVision);
  }

  function renderProfiles() {
    const list = $("profile-list");
    list.innerHTML = "";
    const selected = currentProfile();
    for (const profile of settings.api.profiles) {
      const item = el("button", `model-item ${profile.name === selected.name ? "sel" : ""}`);
      item.type = "button";
      const info = el("span", "mi-info");
      info.append(el("strong", "mi-name", profile.name), el("small", "mi-detail", profile.model || "未填写模型"));
      item.append(el("span", "mi-radio"), info);
      if (profile.supportsVision) item.append(el("span", "mi-kind", "VISION"));
      item.addEventListener("click", async () => {
        settings.api.selected = profile.name;
        await saveSettings(settings);
        renderProfiles();
        fillProfileEditor();
        refreshApiBadge();
        refreshOcrHint();
        renderQuick();
      });
      if (settings.api.profiles.length > 1) {
        const remove = el("button", "mi-edit", "×");
        remove.type = "button";
        remove.title = "删除预设";
        remove.addEventListener("click", async (event) => {
          event.stopPropagation();
          settings.api.profiles = settings.api.profiles.filter((candidate) => candidate !== profile);
          if (settings.api.selected === profile.name) settings.api.selected = settings.api.profiles[0].name;
          await saveSettings(settings);
          renderProfiles();
          fillProfileEditor();
          refreshApiBadge();
          refreshOcrHint();
        });
        item.append(remove);
      }
      list.append(item);
    }
  }

  $("profile-add").addEventListener("click", async () => {
    let index = 1;
    while (settings.api.profiles.some((profile) => profile.name === `预设 ${index}`)) index += 1;
    settings.api.profiles.push({ name: `预设 ${index}`, baseUrl: "", apiKey: "", model: "", supportsVision: false });
    settings.api.selected = `预设 ${index}`;
    await saveSettings(settings);
    renderProfiles();
    fillProfileEditor();
  });

  async function collectProfile() {
    const profile = currentProfile();
    const name = $<HTMLInputElement>("api-name").value.trim() || profile.name;
    if (settings.api.profiles.some((candidate) => candidate !== profile && candidate.name === name)) {
      toast("预设名称已存在", false);
      return false;
    }
    profile.name = name;
    profile.baseUrl = $<HTMLInputElement>("api-base").value.trim();
    profile.model = $<HTMLInputElement>("api-model").value.trim();
    profile.apiKey = $<HTMLInputElement>("api-key").value.trim();
    profile.supportsVision = $<HTMLInputElement>("api-vision").checked;
    settings.api.selected = name;
    if (!profile.supportsVision && settings.ocr.mode === "vision") settings.ocr.mode = "windows";
    await saveSettings(settings);
    renderProfiles();
    refreshApiBadge();
    refreshOcrHint();
    segSet("seg-ocr", settings.ocr.mode);
    renderQuick();
    return true;
  }
  $("api-save").addEventListener("click", async () => { if (await collectProfile()) toast("API 预设已保存"); });
  $("api-test").addEventListener("click", async () => {
    if (!(await collectProfile())) return;
    const profile = activeProfile(settings);
    if (!profile.baseUrl || !profile.model) { toast("请填写 Base URL 和 Model", false); return; }
    const button = $<HTMLButtonElement>("api-test");
    button.disabled = true;
    await llmStream(profile, [{ role: "user", content: "Reply with exactly: OK" }], {
      onDelta: () => {}, onDone: () => { toast("连接成功"); button.disabled = false; },
      onError: (message) => { toast(message, false); button.disabled = false; },
    }, 0);
  });

  function refreshOcrHint() {
    const profile = activeProfile(settings);
    $("ocr-hint").textContent = settings.ocr.mode === "vision"
      ? `使用 ${profile.name} · ${profile.model || "未填写模型"}`
      : "使用 Windows.Media.Ocr，不上传截图";
    $("vision-notice").classList.toggle("warn", settings.ocr.mode === "vision" && !profile.supportsVision);
  }
  document.querySelectorAll("#seg-ocr button").forEach((button) => button.addEventListener("click", async () => {
    const mode = (button as HTMLElement).dataset.v as Settings["ocr"]["mode"];
    if (mode === "vision" && !activeProfile(settings).supportsVision) {
      toast("请先为当前 API 预设开启“模型支持视觉”", false);
      return;
    }
    settings.ocr.mode = mode;
    await saveSettings(settings);
    segSet("seg-ocr", mode);
    refreshOcrHint();
  }));

  function renderPrompts() {
    const prompts = ensurePrompts(settings);
    const list = $("prompt-list");
    list.innerHTML = "";
    const general = el("button", `model-item ${prompts.selected ? "" : "sel"}`);
    general.type = "button";
    general.append(el("span", "mi-radio"), el("span", "mi-info", "通用翻译"));
    general.addEventListener("click", async () => { prompts.selected = ""; await saveSettings(settings); renderPrompts(); renderQuick(); });
    list.append(general);
    for (const prompt of prompts.presets) {
      const row = el("div", `prompt-row ${prompts.selected === prompt.name ? "sel" : ""}`);
      const name = el("input", "f-input") as HTMLInputElement;
      name.value = prompt.name;
      name.placeholder = "提示词名称";
      const text = el("textarea", "f-input f-area") as HTMLTextAreaElement;
      text.value = prompt.text;
      text.placeholder = "例如：医学领域，术语使用规范译法";
      const actions = el("div", "row-end");
      const use = el("button", "btn small tint", prompts.selected === prompt.name ? "使用中" : "使用");
      const remove = el("button", "btn small", "删除");
      use.addEventListener("click", async () => {
        prompt.name = name.value.trim() || prompt.name;
        prompt.text = text.value.trim();
        prompts.selected = prompt.name;
        await saveSettings(settings); renderPrompts(); renderQuick(); toast("提示词已启用");
      });
      remove.addEventListener("click", async () => {
        prompts.presets = prompts.presets.filter((candidate) => candidate !== prompt);
        if (prompts.selected === prompt.name) prompts.selected = "";
        await saveSettings(settings); renderPrompts(); renderQuick();
      });
      actions.append(use, remove); row.append(name, text, actions); list.append(row);
    }
  }
  $("prompt-add").addEventListener("click", async () => {
    const prompts = ensurePrompts(settings);
    let index = 1;
    while (prompts.presets.some((prompt) => prompt.name === `领域 ${index}`)) index += 1;
    prompts.presets.push({ name: `领域 ${index}`, text: "" });
    await saveSettings(settings); renderPrompts();
  });

  function renderHotkeys() {
    document.querySelectorAll<HTMLButtonElement>(".hk-btn").forEach((button) => {
      const key = button.dataset.hk || "";
      button.textContent = settings.hotkeys[key] || HK_DEFAULTS[key];
      button.addEventListener("click", () => {
        recordingHotkey?.button.classList.remove("recording");
        recordingHotkey = { key, button };
        button.classList.add("recording");
        button.textContent = "请按键…";
      }, { once: true });
    });
  }
  document.addEventListener("keydown", async (event) => {
    if (!recordingHotkey) return;
    event.preventDefault(); event.stopPropagation();
    if (event.key === "Escape") { recordingHotkey.button.classList.remove("recording"); recordingHotkey = null; renderHotkeys(); return; }
    const combo = comboFromEvent(event);
    if (!combo) return;
    const duplicate = Object.entries(settings.hotkeys).find(([key, value]) => key !== recordingHotkey!.key && value === combo);
    if (duplicate) { toast("该快捷键已被占用", false); return; }
    settings.hotkeys[recordingHotkey.key] = combo;
    recordingHotkey.button.classList.remove("recording"); recordingHotkey = null;
    await saveSettings(settings); renderHotkeys();
  }, true);
  $("hk-reset").addEventListener("click", async () => { settings.hotkeys = { ...HK_DEFAULTS }; await saveSettings(settings); renderHotkeys(); });

  const bindSegment = (id: string, handler: (value: string) => Promise<void>) => {
    document.querySelectorAll(`#${id} button`).forEach((button) => button.addEventListener("click", () => handler((button as HTMLElement).dataset.v || "")));
  };
  bindSegment("seg-theme", async (value) => { settings.theme = value as Settings["theme"]; applyTheme(value); segSet("seg-theme", value); await saveSettings(settings); });
  bindSegment("seg-scale", async (value) => { settings.uiScale = Number(value); applyScale(settings.uiScale); segSet("seg-scale", value); await saveSettings(settings); });
  bindSegment("seg-stay", async (value) => { settings.popupStaySecs = Number(value); segSet("seg-stay", value); await saveSettings(settings); });
  bindSegment("seg-clickout", async (value) => { settings.closeOnBlur = value === "1"; segSet("seg-clickout", value); await saveSettings(settings); });

  function renderQuick() {
    const box = $("qs-box"); box.innerHTML = "";
    const prompts = ensurePrompts(settings);
    if (prompts.presets.length) {
      const items: [string, string][] = [
        ["", "通用翻译"],
        ...prompts.presets.map((prompt): [string, string] => [prompt.name, prompt.name]),
      ];
      const dropdown = makeDropdown(items, prompts.selected, async (value) => { prompts.selected = value; await saveSettings(settings); });
      dropdown.root.classList.add("up"); box.append(dropdown.root);
    }
    if (settings.api.profiles.length > 1) {
      const dropdown = makeDropdown(settings.api.profiles.map((profile): [string, string] => [profile.name, profile.name]), settings.api.selected, async (value) => {
        settings.api.selected = value; await saveSettings(settings); refreshApiBadge();
      });
      dropdown.root.classList.add("up"); box.append(dropdown.root);
    }
  }
  renderQuick();

  $("btn-chat").addEventListener("click", async () => {
    const chat = await WebviewWindow.getByLabel("chat"); await chat?.show(); await chat?.setFocus();
  });
  onSettingsChanged((next) => {
    settings = next; applyTheme(next.theme); applyScale(next.uiScale); refreshApiBadge(); renderQuick();
  });
}

window.addEventListener("DOMContentLoaded", main);

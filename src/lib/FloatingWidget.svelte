<script lang="ts">
  import { onMount } from "svelte"
  import { invoke, listen } from "./tauri"
  import CompactLanguageDropdown from "./CompactLanguageDropdown.svelte"
  import { LanguageManager, type Language } from "./languages"
  import { applyTheme, updateAutoTheme } from "./utils/theme"
  import {
    ClipboardDocumentIcon,
    XMarkIcon,
    CheckIcon,
  } from "heroicons-svelte/24/outline"

  let config = $state<any>(null)
  let sourceText = $state("")
  let translatedText = $state("")
  let detectedLanguage = $state("")
  let isTranslating = $state(false)
  let errorMessage = $state("")
  let copied = $state(false)
  let copyTimer: ReturnType<typeof setTimeout> | null = null

  // Resolve the configured target language (stored as an English name) to a Language object.
  let targetLanguage = $derived.by<Language>(() => {
    const name = config?.target_language || "English"
    const match = LanguageManager.search(name, false).find(
      (l) => l.english_name.toLowerCase() === name.toLowerCase()
    )
    return (
      match ||
      LanguageManager.findByCode("en") ||
      LanguageManager.createCustomLanguage(name)
    )
  })

  async function translate() {
    if (!sourceText.trim()) {
      translatedText = ""
      detectedLanguage = ""
      return
    }
    isTranslating = true
    errorMessage = ""
    try {
      const result = (await invoke("translate", { text: sourceText })) as {
        detected_language: string
        translated_text: string
        target_language: string
      } | null
      if (result) {
        translatedText = result.translated_text
        detectedLanguage = result.detected_language
      }
    } catch (e) {
      const msg = String(e)
      // The backend rejects rapid duplicates; ignore those quietly.
      if (!msg.toLowerCase().includes("duplicate")) {
        errorMessage = msg
      }
    } finally {
      isTranslating = false
    }
  }

  async function onTargetSelect(language: Language) {
    if (!config) return
    config = { ...config, target_language: language.english_name }
    try {
      await invoke("set_target_language", { language: language.english_name })
    } catch (e) {
      console.error("Failed to set target language:", e)
    }
    // Re-translate the current selection into the newly chosen language.
    await translate()
  }

  async function copyTranslation() {
    if (!translatedText) return
    try {
      await invoke("copy_to_clipboard", { text: translatedText })
      copied = true
      if (copyTimer) clearTimeout(copyTimer)
      copyTimer = setTimeout(() => (copied = false), 1500)
    } catch (e) {
      console.error("Failed to copy translation:", e)
    }
  }

  async function hideWindow() {
    try {
      await invoke("hide_floating_window")
    } catch (e) {
      console.error("Failed to hide floating window:", e)
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      hideWindow()
    }
  }

  onMount(() => {
    // Keep this window's document transparent so the rounded card is what shows.
    document.documentElement.classList.add("gpt-floating")
    document.body.classList.add("gpt-floating")

    const init = async () => {
      try {
        config = await invoke("get_config")
      } catch {
        config = { target_language: "English", theme: "auto" }
      }
      applyTheme(config?.theme || "auto")
    }
    init()

    const unlistenPromise = listen("selection-text", async (event) => {
      try {
        const fresh = await invoke("get_config")
        if (fresh) {
          config = fresh
          // Keep the floating window's theme in sync with the main window.
          applyTheme(config?.theme || "auto")
        }
      } catch {
        /* keep existing config */
      }
      sourceText = (event.payload as string) ?? ""
      translate()
    }) as Promise<() => void>

    const darkModeMediaQuery = window.matchMedia("(prefers-color-scheme: dark)")
    const handleThemeChange = () => {
      if ((config?.theme || "auto") === "auto") updateAutoTheme()
    }
    darkModeMediaQuery.addEventListener("change", handleThemeChange)
    document.addEventListener("keydown", handleKeydown)

    return () => {
      darkModeMediaQuery.removeEventListener("change", handleThemeChange)
      document.removeEventListener("keydown", handleKeydown)
      if (copyTimer) clearTimeout(copyTimer)
      unlistenPromise.then((unlisten) => unlisten && unlisten()).catch(() => {})
    }
  })
</script>

<div class="h-screen w-screen p-2 bg-transparent">
  <div
    class="card card-border bg-base-100 shadow-xl rounded-xl h-full flex flex-col overflow-hidden"
  >
    <!-- Drag handle / header -->
    <div
      data-tauri-drag-region
      class="flex items-center justify-between px-3 py-2 border-b border-base-300/50 cursor-move"
    >
      <span class="text-xs font-semibold text-base-content/70 select-none">
        GPTranslate
      </span>
      <div class="flex items-center gap-1">
        <CompactLanguageDropdown
          selectedLanguage={targetLanguage}
          favoriteLanguages={config?.favorite_languages || []}
          onLanguageSelect={onTargetSelect}
          label="Target language"
        />
        <button
          type="button"
          class="btn btn-ghost btn-xs btn-circle"
          onclick={hideWindow}
          aria-label="Close"
          title="Close (Esc)"
        >
          <XMarkIcon class="w-4 h-4" />
        </button>
      </div>
    </div>

    <!-- Body -->
    <div class="flex-1 min-h-0 overflow-auto px-3 py-2 space-y-2">
      {#if sourceText}
        <div class="text-[11px] text-base-content/50 line-clamp-2">
          {sourceText}
        </div>
      {/if}

      {#if isTranslating}
        <div class="flex items-center gap-2 text-sm text-base-content/60">
          <span class="loading loading-spinner loading-xs"></span>
          Translating…
        </div>
      {:else if errorMessage}
        <div class="text-sm text-error break-words">{errorMessage}</div>
      {:else if translatedText}
        <div class="text-sm text-base-content whitespace-pre-wrap break-words">
          {translatedText}
        </div>
      {:else}
        <div class="text-sm text-base-content/40">
          Select text in any app and press your translate hotkey.
        </div>
      {/if}
    </div>

    <!-- Footer -->
    <div
      class="flex items-center justify-between px-3 py-1.5 border-t border-base-300/50"
    >
      <span class="text-[11px] text-base-content/50 truncate">
        {#if detectedLanguage}
          {detectedLanguage} → {targetLanguage.english_name}
        {/if}
      </span>
      <button
        type="button"
        class="btn btn-soft btn-xs"
        onclick={copyTranslation}
        disabled={!translatedText}
      >
        {#if copied}
          <CheckIcon class="w-3.5 h-3.5" /> Copied
        {:else}
          <ClipboardDocumentIcon class="w-3.5 h-3.5" /> Copy
        {/if}
      </button>
    </div>
  </div>
</div>

import type { AppLanguage, AppTheme, CustomThemeColors } from "../types";

export const THEME_STORAGE_KEY = "ikoch-theme";
export const CUSTOM_THEME_STORAGE_KEY = "ikoch-custom-theme";
export const LANGUAGE_STORAGE_KEY = "ikoch-lang";

export const DEFAULT_CUSTOM_THEME: CustomThemeColors = {
  bg_app: "#090B12",
  bg_panel: "#111522",
  accent: "#6366F1",
  text_primary: "#F1F5F9",
};

export interface ThemePresetMeta {
  id: Exclude<AppTheme, "custom">;
  name: string;
  descRu: string;
  descEn: string;
  bgApp: string;
  bgPanel: string;
  bgCard: string;
  accent: string;
  textPrimary: string;
}

export const THEME_PRESETS: ThemePresetMeta[] = [
  {
    id: "ikoch_dark",
    name: "iKoch Dark",
    descRu: "Тёмный графит и фиолетово-синий",
    descEn: "Dark graphite & indigo blue",
    bgApp: "#080A0F",
    bgPanel: "#0B0D14",
    bgCard: "#121623",
    accent: "#4F6EF7",
    textPrimary: "#F1F5F9",
  },
  {
    id: "oled_midnight",
    name: "OLED Midnight",
    descRu: "Глубокий чёрный и неоновый голубой",
    descEn: "True black & neon cyan",
    bgApp: "#050608",
    bgPanel: "#080A0E",
    bgCard: "#0E1218",
    accent: "#06B6D4",
    textPrimary: "#F1F5F9",
  },
  {
    id: "crimson_pro",
    name: "Crimson Pro",
    descRu: "Тёмный уголь и малиново-красный",
    descEn: "Dark charcoal & crimson",
    bgApp: "#0B080B",
    bgPanel: "#100C10",
    bgCard: "#181218",
    accent: "#E11D48",
    textPrimary: "#F8F4F6",
  },
  {
    id: "nordic_slate",
    name: "Nordic Slate",
    descRu: "Холодный серо-синий и изумрудный",
    descEn: "Cool slate & emerald",
    bgApp: "#0C1118",
    bgPanel: "#101721",
    bgCard: "#16202D",
    accent: "#10B981",
    textPrimary: "#ECFDF5",
  },
  {
    id: "amethyst_neon",
    name: "Amethyst Neon",
    descRu: "Глубокий сливовый и неоновый аметист",
    descEn: "Deep plum & vivid violet",
    bgApp: "#090711",
    bgPanel: "#0F0C1B",
    bgCard: "#171229",
    accent: "#A855F7",
    textPrimary: "#F5F3FF",
  },
  {
    id: "sunset_amber",
    name: "Sunset Amber",
    descRu: "Тёплый графит и золотой янтарь",
    descEn: "Warm charcoal & amber gold",
    bgApp: "#0C0907",
    bgPanel: "#120E0B",
    bgCard: "#1B1510",
    accent: "#F59E0B",
    textPrimary: "#FEF3C7",
  },
  {
    id: "emerald_matrix",
    name: "Emerald Matrix",
    descRu: "Карбон-чёрный и кибер-зелёный",
    descEn: "Carbon black & cyber green",
    bgApp: "#060B08",
    bgPanel: "#0A110D",
    bgCard: "#101B14",
    accent: "#22C55E",
    textPrimary: "#ECFDF5",
  },
  {
    id: "frost_slate",
    name: "Frost Slate",
    descRu: "Морозный грифель и небесный лёд",
    descEn: "Frost slate & arctic sky",
    bgApp: "#0A0F17",
    bgPanel: "#0F1622",
    bgCard: "#162030",
    accent: "#38BDF8",
    textPrimary: "#F0F9FF",
  },
];

const CUSTOM_VAR_NAMES = [
  "--bg-app",
  "--bg-panel",
  "--bg-card",
  "--bg-elevated",
  "--accent",
  "--accent-hover",
  "--accent-rgb",
  "--accent-soft",
  "--text-primary",
] as const;

export function isValidHex(hex: string | undefined | null): hex is string {
  return Boolean(hex && /^#[0-9a-fA-F]{6}$/.test(hex.trim()));
}

function hexToRgb(hex: string): [number, number, number] {
  const clean = hex.replace("#", "");
  return [
    parseInt(clean.slice(0, 2), 16),
    parseInt(clean.slice(2, 4), 16),
    parseInt(clean.slice(4, 6), 16),
  ];
}

function rgbToHex(r: number, g: number, b: number): string {
  const clamp = (v: number) =>
    Math.max(0, Math.min(255, Math.round(v)))
      .toString(16)
      .padStart(2, "0");
  return `#${clamp(r)}${clamp(g)}${clamp(b)}`;
}

function mixHex(baseHex: string, targetHex: string, ratio: number): string {
  const [r1, g1, b1] = hexToRgb(baseHex);
  const [r2, g2, b2] = hexToRgb(targetHex);
  return rgbToHex(
    r1 + (r2 - r1) * ratio,
    g1 + (g2 - g1) * ratio,
    b1 + (b2 - b1) * ratio
  );
}

export function normalizeThemeSlug(raw: string | null | undefined): AppTheme {
  const valid: AppTheme[] = [
    "ikoch_dark",
    "oled_midnight",
    "crimson_pro",
    "nordic_slate",
    "amethyst_neon",
    "sunset_amber",
    "emerald_matrix",
    "frost_slate",
    "custom",
  ];
  return valid.includes(raw as AppTheme)
    ? (raw as AppTheme)
    : "ikoch_dark";
}

export function normalizeCustomColors(
  colors?: Partial<CustomThemeColors> | null
): CustomThemeColors {
  return {
    bg_app: isValidHex(colors?.bg_app)
      ? colors.bg_app.trim().toUpperCase()
      : DEFAULT_CUSTOM_THEME.bg_app,
    bg_panel: isValidHex(colors?.bg_panel)
      ? colors.bg_panel.trim().toUpperCase()
      : DEFAULT_CUSTOM_THEME.bg_panel,
    accent: isValidHex(colors?.accent)
      ? colors.accent.trim().toUpperCase()
      : DEFAULT_CUSTOM_THEME.accent,
    text_primary: isValidHex(colors?.text_primary)
      ? colors.text_primary.trim().toUpperCase()
      : DEFAULT_CUSTOM_THEME.text_primary,
  };
}

export function applyThemeToDocument(
  theme: AppTheme,
  custom?: CustomThemeColors | null
): void {
  const root = document.documentElement;
  root.dataset.theme = theme;

  if (theme !== "custom") {
    for (const v of CUSTOM_VAR_NAMES) {
      root.style.removeProperty(v);
    }
    return;
  }

  const c = normalizeCustomColors(custom);
  const [ar, ag, ab] = hexToRgb(c.accent);
  const bgCard = mixHex(c.bg_panel, "#FFFFFF", 0.05);
  const bgElevated = mixHex(c.bg_panel, "#FFFFFF", 0.1);
  const accentHover = mixHex(c.accent, "#FFFFFF", 0.18);

  root.style.setProperty("--bg-app", c.bg_app);
  root.style.setProperty("--bg-panel", c.bg_panel);
  root.style.setProperty("--bg-card", bgCard);
  root.style.setProperty("--bg-elevated", bgElevated);
  root.style.setProperty("--accent", c.accent);
  root.style.setProperty("--accent-hover", accentHover);
  root.style.setProperty("--accent-rgb", `${ar} ${ag} ${ab}`);
  root.style.setProperty("--accent-soft", `rgba(${ar}, ${ag}, ${ab}, 0.16)`);
  root.style.setProperty("--text-primary", c.text_primary);
}

export function loadStoredTheme(): {
  theme: AppTheme;
  custom: CustomThemeColors;
} {
  const rawTheme = window.localStorage.getItem(THEME_STORAGE_KEY);
  let custom = DEFAULT_CUSTOM_THEME;
  try {
    const rawCustom = window.localStorage.getItem(CUSTOM_THEME_STORAGE_KEY);
    if (rawCustom) {
      custom = normalizeCustomColors(
        JSON.parse(rawCustom) as Partial<CustomThemeColors>
      );
    }
  } catch {
    custom = DEFAULT_CUSTOM_THEME;
  }
  return { theme: normalizeThemeSlug(rawTheme), custom };
}

export function saveStoredTheme(
  theme: AppTheme,
  custom?: CustomThemeColors | null
): void {
  window.localStorage.setItem(THEME_STORAGE_KEY, theme);
  if (custom) {
    window.localStorage.setItem(
      CUSTOM_THEME_STORAGE_KEY,
      JSON.stringify(normalizeCustomColors(custom))
    );
  }
}

export function loadStoredLanguage(): AppLanguage {
  const saved = window.localStorage.getItem(LANGUAGE_STORAGE_KEY);
  return saved === "en" ? "en" : "ru";
}

export function saveStoredLanguage(lang: AppLanguage): void {
  window.localStorage.setItem(LANGUAGE_STORAGE_KEY, lang);
}

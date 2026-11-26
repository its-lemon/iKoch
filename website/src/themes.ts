export interface ThemePreset {
  id: string;
  name: string;
  descRu: string;
  descEn: string;
  bgApp: string;
  bgPanel: string;
  bgCard: string;
  accent: string;
  textPrimary: string;
}

export const THEME_PRESETS: ThemePreset[] = [
  {
    id: "ikoch_dark",
    name: "iKoch Classic",
    descRu: "Тёмный графит и индиго",
    descEn: "Dark graphite & indigo",
    bgApp: "#080A0F",
    bgPanel: "#0D1018",
    bgCard: "#131826",
    accent: "#4F6EF7",
    textPrimary: "#F1F5F9",
  },
  {
    id: "oled_midnight",
    name: "Pure Black (OLED)",
    descRu: "Чистый чёрный и неоновый циан",
    descEn: "True black & neon cyan",
    bgApp: "#020305",
    bgPanel: "#080A0E",
    bgCard: "#0F131A",
    accent: "#06B6D4",
    textPrimary: "#F1F5F9",
  },
  {
    id: "crimson_pro",
    name: "Crimson",
    descRu: "Тёмный уголь и малиново-красный",
    descEn: "Dark charcoal & crimson",
    bgApp: "#0B080B",
    bgPanel: "#120D12",
    bgCard: "#1A131A",
    accent: "#E11D48",
    textPrimary: "#F8F4F6",
  },
  {
    id: "nordic_slate",
    name: "Emerald",
    descRu: "Глубокий графит и изумрудный",
    descEn: "Deep graphite & emerald",
    bgApp: "#070D0A",
    bgPanel: "#0D1612",
    bgCard: "#13201A",
    accent: "#10B981",
    textPrimary: "#ECFDF5",
  },
  {
    id: "frost_slate",
    name: "Ocean Blue",
    descRu: "Глубокий океан и небесно-синий",
    descEn: "Deep ocean & sky blue",
    bgApp: "#080E17",
    bgPanel: "#0E1624",
    bgCard: "#152134",
    accent: "#38BDF8",
    textPrimary: "#F0F9FF",
  },
  {
    id: "amethyst_neon",
    name: "Violet Neon",
    descRu: "Тёмный сливовый и фиолетовый неон",
    descEn: "Deep plum & vivid violet",
    bgApp: "#090711",
    bgPanel: "#100D1D",
    bgCard: "#18132B",
    accent: "#A855F7",
    textPrimary: "#F5F3FF",
  },
  {
    id: "sunset_amber",
    name: "Warm Amber",
    descRu: "Тёплый уголь и золотой янтарь",
    descEn: "Warm charcoal & amber gold",
    bgApp: "#0C0907",
    bgPanel: "#14100C",
    bgCard: "#1D1712",
    accent: "#F59E0B",
    textPrimary: "#FEF3C7",
  },
  {
    id: "emerald_matrix",
    name: "Steel Slate",
    descRu: "Холодный стальной и нейтральный серый",
    descEn: "Cool steel slate & silver",
    bgApp: "#0B0E14",
    bgPanel: "#121720",
    bgCard: "#19202C",
    accent: "#64748B",
    textPrimary: "#F1F5F9",
  },
];

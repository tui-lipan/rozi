import DefaultTheme from "vitepress/theme";
import type { Theme } from "vitepress";

import CaptureGallery from "./CaptureGallery.vue";
import Layout from "./Layout.vue";
import "./style.css";
import "./landing.css";
import "./gallery.css";

export default {
  extends: DefaultTheme,
  Layout,
  enhanceApp({ app }) {
    app.component("CaptureGallery", CaptureGallery);
  },
} satisfies Theme;

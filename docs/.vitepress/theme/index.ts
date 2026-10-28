import DefaultTheme from "vitepress/theme";
import type { Theme } from "vitepress";

import CaptureGallery from "./CaptureGallery.vue";
import ExtensionGallery from "./ExtensionGallery.vue";
import Layout from "./Layout.vue";
import "./style.css";
import "./landing.css";
import "./gallery.css";
import "./extensions.css";

export default {
  extends: DefaultTheme,
  Layout,
  enhanceApp({ app }) {
    app.component("CaptureGallery", CaptureGallery);
    app.component("ExtensionGallery", ExtensionGallery);
  },
} satisfies Theme;

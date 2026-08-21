<script setup lang="ts">
import DefaultTheme from "vitepress/theme";
import { useRoute } from "vitepress";
import { computed } from "vue";
import Landing from "./Landing.vue";
import NavGitHubLink from "./NavGitHubLink.vue";
import NavTitleMeta from "./NavTitleMeta.vue";

const { Layout: DefaultLayout } = DefaultTheme;
const route = useRoute();

/**
 * `/` is the landing page, which owns its own chrome rather than reusing the
 * docs navbar and sidebar. Every other route is a normal VitePress doc page.
 */
const isLanding = computed(
  () => route.path === "/" || route.path === "/index.html",
);
</script>

<template>
  <Landing v-if="isLanding" />
  <template v-else>
    <!-- The landing page's decoration, quieter and kept out of the reading
         column - see `.doc-bg` in style.css. Fixed, so it costs no layout. -->
    <div class="doc-bg" aria-hidden="true">
      <span class="r1"></span>
      <span class="r2"></span>
      <span class="r3"></span>
      <span class="r4"></span>
      <span class="r5"></span>
      <span class="r6"></span>
      <span class="r7"></span>
      <span class="l1"></span>
      <span class="l2"></span>
      <span class="l3"></span>
      <span class="l4"></span>
    </div>
    <DefaultLayout>
      <template #nav-bar-title-after><NavTitleMeta /></template>
      <template #nav-bar-content-after><NavGitHubLink /></template>
    </DefaultLayout>
  </template>
</template>

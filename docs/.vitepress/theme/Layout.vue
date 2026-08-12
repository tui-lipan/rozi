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
  <DefaultLayout v-else>
    <template #nav-bar-title-after><NavTitleMeta /></template>
    <template #nav-bar-content-after><NavGitHubLink /></template>
  </DefaultLayout>
</template>

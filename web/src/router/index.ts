import { createRouter, createWebHistory, type RouteRecordRaw } from 'vue-router'

/**
 * Route table. Every view is a lazy chunk, so the shell and its translations are
 * the only eagerly loaded code; the heavy renderers (echarts, BabylonJS) arrive
 * with the views that use them.
 */
const routes: RouteRecordRaw[] = [
  { path: '/', redirect: '/maps' },
  { path: '/maps', name: 'map-list', component: () => import('@/views/MapListView.vue') },
  { path: '/maps/:id', name: 'map-viewer', component: () => import('@/views/MapViewerView.vue') },
  { path: '/run', name: 'run', component: () => import('@/views/RunView.vue') },
  { path: '/runs', name: 'runs', component: () => import('@/views/SimulationView.vue') },
  { path: '/runs/:id', name: 'simulation', component: () => import('@/views/SimulationView.vue') },
  { path: '/settings', name: 'settings', component: () => import('@/views/SettingsView.vue') },
  {
    path: '/:pathMatch(.*)*',
    name: 'not-found',
    component: () => import('@/views/NotFoundView.vue'),
  },
]

export const router = createRouter({
  history: createWebHistory(import.meta.env.BASE_URL),
  routes,
  scrollBehavior: () => ({ top: 0 }),
})

import { createRouter, createWebHistory, type RouteRecordRaw } from 'vue-router'

/**
 * Route table. Every view is a lazy chunk, so the shell and its translations are
 * the only eagerly loaded code; the heavy renderers (echarts, BabylonJS) arrive
 * with the views that use them.
 */
const routes: RouteRecordRaw[] = [
  { path: '/', name: 'dashboard', component: () => import('@/views/DashboardView.vue') },
  { path: '/maps', name: 'map-list', component: () => import('@/views/MapListView.vue') },
  { path: '/maps/:id', name: 'map-viewer', component: () => import('@/views/MapViewerView.vue') },
  {
    path: '/maps/:id/studio',
    name: 'map-studio',
    component: () => import('@/views/MapStudioView.vue'),
  },
  { path: '/routes', name: 'route-studio', component: () => import('@/views/RouteStudioView.vue') },
  {
    path: '/simulations/:id',
    name: 'simulation',
    component: () => import('@/views/SimulationView.vue'),
  },
  {
    path: '/simulations/:id/trajectory',
    name: 'trajectory',
    component: () => import('@/views/TrajectoryView.vue'),
  },
  {
    path: '/simulations/:id/sensors',
    name: 'sensor',
    component: () => import('@/views/SensorView.vue'),
  },
  {
    path: '/simulations/:id/audit',
    name: 'audit',
    component: () => import('@/views/AuditView.vue'),
  },
  { path: '/batch', name: 'batch', component: () => import('@/views/BatchView.vue') },
  { path: '/omf', name: 'omf-inspector', component: () => import('@/views/OmfInspectorView.vue') },
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

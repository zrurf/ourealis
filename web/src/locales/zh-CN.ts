/**
 * Simplified Chinese message catalog.
 *
 * The annotation ties the key set to `en.ts` at build time; values stay in the
 * units and vocabulary of the domain (metres, seconds, radians).
 */
import map from './parts/zh-CN/map'
import omf from './parts/zh-CN/omf'
import simulation from './parts/zh-CN/simulation'
const zhCN: typeof import('./en').default = {
  // 各领域命名空间，分文件维护后在此合并。
  ...map,
  ...simulation,
  ...omf,

  app: {
    title: 'Ourealis',
  },
  nav: {
    dashboard: '总览',
    maps: '地图',
    routes: '路线',
    batch: '群体',
    omf: 'OMF 检视',
    settings: '设置',
  },
  theme: {
    label: '主题',
    light: '亮色',
    dark: '暗色',
  },
  language: {
    label: '语言',
    en: 'English',
    'zh-CN': '简体中文',
  },
  common: {
    empty: '暂无可显示的内容。',
    loading: '加载中…',
    error: '出错了。',
    retry: '重试',
    cancel: '取消',
    confirm: '确定',
    save: '保存',
    delete: '删除',
    close: '关闭',
    refresh: '刷新',
    search: '搜索',
    copy: '复制',
    back: '返回',
  },
  error: {
    fromService: '来自服务端',
  },
  views: {
    dashboard: { title: '总览' },
    mapList: { title: '地图库' },
    mapViewer: { title: '地图预览' },
    mapStudio: { title: '地图制作' },
    routeStudio: { title: '路线工作室' },
    simulation: { title: '模拟' },
    trajectory: { title: '轨迹' },
    sensor: { title: '传感器' },
    audit: { title: '审计' },
    batch: { title: '群体批量' },
    omf: { title: 'OMF 检视' },
    settings: { title: '设置' },
    notFound: { title: '页面不存在' },
  },
}

export default zhCN

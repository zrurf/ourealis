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

  tasks: {
    tray: '后台任务（{count}）',
    empty: '当前没有任务在运行。',
    clearFinished: '清除已完成',
    elapsed: '已运行 {seconds} 秒',
    slowHint: '大地图或精细分辨率下这一步较慢，服务仍在计算。',
    done: '{what}已完成。',
    failed: '{what}失败。',
    cancelled: '任务已取消。',
    kind: {
      simulation: '模拟',
      route_preview: '路线预览',
      route_plan: '路线规划',
      synthetic_map: '地图生成',
    },
    state: {
      queued: '排队中',
      running: '运行中',
      succeeded: '已完成',
      failed: '失败',
      cancelled: '已取消',
    },
    labels: {
      preview: '正在规划候选路线',
      plan: '正在规划路线与限速曲线',
      synthetic: '正在生成地图',
      run: '正在运行模拟',
    },
  },
  run: {
    stage: {
      map: '地图',
      route: '路线',
      runner: '跑者',
      sensors: '传感器',
      run: '运行',
    },
    stages: '运行阶段',
    progress: '距离可运行配置的进度',
    blocked: {
      noMap: '请先选择地图',
      noStart: '请先放置起点',
      noGoal: '请先放置终点',
    },
    mode: { simple: '简单', expert: '专家' },
    changed: '已改 {count} 项',
    route: {
      hint: '在地图上单击放置起点与终点，拖动标记即可移动；双击标记可删除。',
      armed: '在地图上单击以放置已选中的点。',
      refused: '{count} 个点不可用：{reason}。',
      reason: {
        forbidden: '该处被地图标记为禁行',
        outside: '该点在地图范围之外',
        too_close: '距离障碍仅 {distance} 米，跑步者无法通过',
        unknown: '地图未标注',
      },
    },
    plan: {
      title: '规划',
      planning: '规划中…',
      took: '规划耗时 {ms} 毫秒',
      length: '长度',
      duration: '预计用时',
      pathRatio: '路径比',
      candidates: '候选数',
      empty: '路线画好后会自动规划。',
      chosen: '已选中',
      inspectHint: '规划器按路线与随机种子挑选。点击某一行只改变显示，不改变实际运行的那条。',
    },
    recipe: {
      campusJog: '校园慢跑',
      campusJogHint: '中等强度，其余全部使用默认设置。',
      track: '场地间歇',
      trackHint: '绕圈 4 圈、竞速配速、前快后慢。',
      phone: '手机 + 手表',
      phoneHint: 'GNSS 1 Hz、惯性 100 Hz，事件可重复触发。',
      truth: '纯净真值',
      truthHint: '关闭传感器噪声与指标，只要轨迹本身。',
    },
    ready: '就绪',
    canvasLabel: '地图：单击放置路线点，Tab 切换选中点，方向键移动选中点',
    menu: {
      setStart: '将起点设在此处{replace}',
      setGoal: '将终点设在此处{replace}',
      setReference: '将参考点设在此处',
      addWaypoint: '在此处添加途经点',
      replace: '（替换当前的点）',
      remove: '删除{what}',
      clear: '清除{what}',
    },
    handle: {
      selected: '已选中{role}，东 {x} 米，北 {y} 米',
      role: {
        start: '起点',
        goal: '终点',
        reference: '参考点',
        waypoint: '途经点',
        checkpoint: '打卡点',
      },
    },
    submit: '开始模拟',
  },
  app: {
    title: 'Ourealis',
  },
  nav: {
    collapse: '收起侧栏',
    expand: '展开侧栏',

    dashboard: '总览',
    maps: '地图',
    run: '新建运行',
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
    yes: '是',
    no: '否',
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
    run: { title: '运行工作区' },
    mapViewer: { title: '地图预览' },
    mapStudio: { title: '地图制作' },
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

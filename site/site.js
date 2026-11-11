const toggle = document.querySelector('.theme-toggle');
const systemTheme = window.matchMedia('(prefers-color-scheme: dark)');
const isDark = () => document.documentElement.dataset.theme
  ? document.documentElement.dataset.theme === 'dark'
  : systemTheme.matches;
function updateThemeControl() {
  const dark = isDark();
  toggle.setAttribute('aria-label', `切换为${dark ? '浅' : '深'}色外观`);
  toggle.title = `切换为${dark ? '浅' : '深'}色外观`;
  document.querySelector('meta[name="theme-color"]').content = dark ? '#242724' : '#f8f9f6';
}
if (toggle) {
  toggle.hidden = false;
  updateThemeControl();
  toggle.addEventListener('click', () => {
    const next = isDark() ? 'light' : 'dark';
    document.documentElement.dataset.theme = next;
    try { localStorage.setItem('easycopy-site-theme', next); } catch { /* Appearance still works without storage. */ }
    updateThemeControl();
  });
  systemTheme.addEventListener('change', updateThemeControl);
}

// Apply saved appearance before first paint. Downloads never depend on JavaScript.
try {
  const theme = localStorage.getItem('easycopy-site-theme');
  if (theme === 'light' || theme === 'dark') document.documentElement.dataset.theme = theme;
} catch { /* Private browsing may disable storage; use the system preference. */ }

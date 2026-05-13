(function () {
  // Hamburger toggle
  const ham = document.querySelector('.hamburger');
  const sidebar = document.querySelector('.sidebar');
  if (ham && sidebar) {
    ham.addEventListener('click', () => sidebar.classList.toggle('open'));
    document.addEventListener('click', (e) => {
      if (!sidebar.contains(e.target) && !ham.contains(e.target)) {
        sidebar.classList.remove('open');
      }
    });
  }

  // Active link highlighting
  const currentPage = location.pathname.split('/').pop() || 'index.html';
  document.querySelectorAll('.sidebar a').forEach((a) => {
    const href = a.getAttribute('href');
    if (href && (href === currentPage || href.endsWith('/' + currentPage))) {
      a.classList.add('active');
    }
  });
})();

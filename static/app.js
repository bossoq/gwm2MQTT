async function apiPost(url, data) {
  try {
    const res = await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(data),
    });
    return await res.json();
  } catch (e) {
    return { success: false, message: 'Network error: ' + e.message };
  }
}

function showToast(message, type) {
  const container = document.getElementById('toasts');
  if (!container) return;
  const toast = document.createElement('div');
  toast.className = 'toast ' + (type || '');
  toast.textContent = message;
  container.appendChild(toast);
  setTimeout(() => {
    toast.style.opacity = '0';
    toast.style.transform = 'translateX(110%)';
    setTimeout(() => toast.remove(), 350);
  }, 3500);
}

// withGlobalTauri = true => window.__TAURI__ disponivel (sem bundler)
document.getElementById("btn").addEventListener("click", async () => {
  const out = document.getElementById("out");
  try {
    out.textContent = await window.__TAURI__.core.invoke("ping", { name: "Bookferry" });
  } catch (e) {
    out.textContent = "erro: " + e;
  }
});

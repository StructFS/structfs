import initWasm, { init, execute } from "/wasm/pkg/structfs_wasm.js";

const output = document.getElementById("repl-output");
const input = document.getElementById("repl-input");
const status = document.getElementById("wasm-status");
const playground = document.getElementById("playground");
const resetBtn = document.getElementById("btn-reset");

let history = [];
let historyPos = -1;

function appendLine(text, cls) {
  const line = document.createElement("div");
  line.className = cls;
  line.textContent = text;
  output.appendChild(line);
  output.scrollTop = output.scrollHeight;
}

function runCommand(cmd) {
  if (!cmd.trim()) return;

  appendLine("> " + cmd, "line-prompt");

  const result = execute(cmd);
  if (result) {
    const cls = result.startsWith("Error:") ? "line-error" : "line-output";
    // Split multiline output
    result.split("\n").forEach((line) => appendLine(line, cls));
  }

  history.unshift(cmd);
  historyPos = -1;
}

async function main() {
  try {
    await initWasm("/wasm/pkg/structfs_wasm_bg.wasm");
    init();

    status.style.display = "none";
    playground.style.display = "block";

    appendLine("StructFS playground — type 'help' for commands", "line-info");
    appendLine("A memory store is mounted at /data.", "line-info");
    appendLine("", "line-info");

    input.focus();
  } catch (e) {
    status.innerHTML =
      '<span class="wasm-error">Failed to load playground: ' + e + "</span>";
    return;
  }

  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      const cmd = input.value;
      input.value = "";
      runCommand(cmd);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      if (historyPos < history.length - 1) {
        historyPos++;
        input.value = history[historyPos];
      }
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      if (historyPos > 0) {
        historyPos--;
        input.value = history[historyPos];
      } else {
        historyPos = -1;
        input.value = "";
      }
    }
  });

  resetBtn.addEventListener("click", () => {
    init();
    output.innerHTML = "";
    appendLine("Session reset.", "line-info");
    appendLine("", "line-info");
    input.focus();
  });

  document.querySelectorAll(".example-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      const cmd = btn.getAttribute("data-cmd");
      input.value = cmd;
      input.focus();
      runCommand(cmd);
      input.value = "";
    });
  });
}

main();

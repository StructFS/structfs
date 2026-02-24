import init, { encode, decode, is_xid_identifier } from "/wasm/pkg/namecode_wasm.js";

const statusEl = document.getElementById("wasm-status");
const playgroundEl = document.getElementById("playground");
const inputEl = document.getElementById("input-text");
const encodedEl = document.getElementById("output-encoded");
const decodedEl = document.getElementById("output-decoded");
const decodePaneEl = document.getElementById("decode-pane");
const propXid = document.getElementById("prop-xid");

function update() {
  const input = inputEl.value;

  // Encode
  encodedEl.value = encode(input);

  // XID check
  const isXid = is_xid_identifier(input);
  propXid.textContent = isXid ? "yes" : "no";
  propXid.className = "prop-val " + (isXid ? "yes" : "no");

  // Decode
  const result = decode(input);
  if (result.startsWith("Error: ")) {
    if (isXid) {
      // Valid XID that isn't namecode — decode is identity
      decodedEl.value = input;
      decodePaneEl.classList.remove("inactive");
    } else {
      decodedEl.value = "";
      decodePaneEl.classList.add("inactive");
    }
  } else {
    decodedEl.value = result;
    decodePaneEl.classList.remove("inactive");
  }
}

async function main() {
  try {
    await init("/wasm/pkg/namecode_wasm_bg.wasm");
    statusEl.style.display = "none";
    playgroundEl.style.display = "block";

    inputEl.addEventListener("input", update);

    document.getElementById("use-encoded").addEventListener("click", () => {
      if (encodedEl.value) {
        inputEl.value = encodedEl.value;
        update();
      }
    });

    document.getElementById("use-decoded").addEventListener("click", () => {
      if (decodedEl.value) {
        inputEl.value = decodedEl.value;
        update();
      }
    });

    update();
  } catch (e) {
    statusEl.textContent = "Failed to load WASM: " + e.message;
    statusEl.className = "wasm-error";
    console.error(e);
  }
}

main();

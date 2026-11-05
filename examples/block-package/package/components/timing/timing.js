// Draws a block's steps as a bar below the list, each step as wide as its minutes. The picture is taken when
// this function returns. Text goes in as text: a document is data, never markup or script.

function minutesOf(text) {
  const found = /(\d+)\s*min/.exec(text);
  return found ? Number(found[1]) : 1;
}

export default function draw(block) {
  const items = block.querySelectorAll('[data-field="steps"] [data-item]');
  const bar = document.createElement("div");
  bar.className = "timing-bar";
  let total = 0;
  for (const item of items) {
    const minutes = minutesOf(item.textContent);
    total += minutes;
    const step = document.createElement("div");
    step.className = "timing-step";
    step.style.flexGrow = String(minutes);
    step.textContent = String(minutes);
    bar.append(step);
  }
  const sum = document.createElement("p");
  sum.className = "timing-total";
  sum.textContent = `${total} min in all`;
  block.append(bar, sum);
}

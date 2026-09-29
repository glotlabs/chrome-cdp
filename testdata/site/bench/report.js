// The report loader: its data has no `summary` for the current quarter,
// and the code does not expect that.
const DATA = {
  quarters: {
    Q1: { summary: { total: 120400, orders: 311 } },
    Q2: { summary: { total: 98100, orders: 254 } },
    Q3: {},
  },
};

function renderQuarter(name) {
  const q = DATA.quarters[name];
  return `<p>${name}: $${q.summary.total.toLocaleString('en-US')} from ${q.summary.orders} orders</p>`;
}

document.getElementById('load').addEventListener('click', () => {
  const out = document.getElementById('out');
  out.innerHTML = '';
  out.innerHTML = ['Q1', 'Q2', 'Q3'].map(renderQuarter).join('');
});

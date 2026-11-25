if (location.host !== host || (protocol === 'https:' && location.protocol !== protocol)) return 0;
const usable = (el) => !el.disabled && !el.readOnly && el.getClientRects().length > 0;
const inputs = [...document.querySelectorAll('input')].filter(usable);
const secret =
  inputs.find((el) => el.type === 'password' && el.autocomplete !== 'new-password') ??
  inputs.find((el) => el.type === 'password');
const textual = (el) => ['text', 'email', 'tel'].includes(el.type);
const precedes = (el) => !secret || el.compareDocumentPosition(secret) & Node.DOCUMENT_POSITION_FOLLOWING;
const candidates = inputs.filter((el) => textual(el) && precedes(el));
const sameForm = secret?.form ? candidates.filter((el) => el.form === secret.form) : [];
const pool = sameForm.length ? sameForm : candidates;
const declared = (el) => /\b(username|email)\b/.test(el.autocomplete) || el.type === 'email';
const hinted = (el) => /user|login|mail|account|benutzer|konto/i.test(`${el.name} ${el.id}`);
const name = pool.find(declared) ?? pool.find(hinted) ?? (secret ? pool.at(-1) : undefined);
const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
const put = (el, value) => {
  el.focus();
  setValue.call(el, value);
  el.dispatchEvent(new Event('input', { bubbles: true }));
  el.dispatchEvent(new Event('change', { bubbles: true }));
};
let filled = 0;
if (name && user) {
  put(name, user);
  filled += 1;
}
if (secret && password) {
  put(secret, password);
  filled += 1;
}
return filled;

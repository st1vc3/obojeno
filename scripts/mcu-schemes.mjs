// Prints {variant: {light: {role: hex}, dark: {role: hex}}} for a source color,
// computed by Google's official material-color-utilities (spec 2021, phone).
// Usage: node mcu-schemes.mjs '#rrggbb'
import * as mcu from '@material/material-color-utilities';

const variants = {
  'content': mcu.SchemeContent,
  'tonal-spot': mcu.SchemeTonalSpot,
  'expressive': mcu.SchemeExpressive,
  'fidelity': mcu.SchemeFidelity,
  'fruit-salad': mcu.SchemeFruitSalad,
  'monochrome': mcu.SchemeMonochrome,
  'neutral': mcu.SchemeNeutral,
  'rainbow': mcu.SchemeRainbow,
  'vibrant': mcu.SchemeVibrant,
};

const source = mcu.Hct.fromInt(mcu.argbFromHex(process.argv[2]));
const colors = new mcu.MaterialDynamicColors().allColors;
const out = {};
for (const [name, Scheme] of Object.entries(variants)) {
  out[name] = {};
  for (const [mode, isDark] of [['light', false], ['dark', true]]) {
    const scheme = new Scheme(source, isDark, 0.0, '2021', 'phone');
    out[name][mode] = Object.fromEntries(
      colors.map((c) => [c.name, mcu.hexFromArgb(c.getArgb(scheme))]),
    );
  }
}
console.log(JSON.stringify(out));

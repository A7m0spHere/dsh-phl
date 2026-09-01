/** @type {import('tailwindcss').Config} */
const withOpacity = (variable) => `hsl(var(${variable}) / <alpha-value>)`

export default {
  darkMode: 'class',
  content: ['./index.html', './src/**/*.{ts,tsx}'],
  theme: {
    extend: {
      colors: {
        canvas: withOpacity('--c-canvas'),
        chrome: withOpacity('--c-chrome'),
        surface: {
          DEFAULT: withOpacity('--c-surface'),
          raised: withOpacity('--c-surface-raised'),
          sunken: withOpacity('--c-surface-sunken'),
          hover: withOpacity('--c-surface-hover'),
        },
        line: {
          DEFAULT: withOpacity('--c-line'),
          strong: withOpacity('--c-line-strong'),
        },
        ink: {
          DEFAULT: withOpacity('--c-ink'),
          muted: withOpacity('--c-ink-muted'),
          faint: withOpacity('--c-ink-faint'),
        },
        accent: {
          DEFAULT: withOpacity('--c-accent'),
          hover: withOpacity('--c-accent-hover'),
          press: withOpacity('--c-accent-press'),
          soft: withOpacity('--c-accent-soft'),
          ink: withOpacity('--c-accent-ink'),
          on: withOpacity('--c-accent-on'),
        },
        ok: withOpacity('--c-ok'),
        warn: withOpacity('--c-warn'),
        danger: withOpacity('--c-danger'),
        info: withOpacity('--c-info'),
      },
      borderRadius: {
        xs: '3px',
        sm: '5px',
        DEFAULT: '6px',
        md: '6px',
        lg: '8px',
        xl: '11px',
      },
      fontFamily: {
        sans: [
          'Inter var',
          'Inter',
          '-apple-system',
          'Segoe UI Variable Display',
          'Segoe UI',
          'PingFang SC',
          'Microsoft YaHei UI',
          'Noto Sans SC',
          'system-ui',
          'sans-serif',
        ],
        mono: ['JetBrains Mono', 'Cascadia Mono', 'Consolas', 'SFMono-Regular', 'monospace'],
      },
      fontSize: {
        '2xs': ['10px', { lineHeight: '13px', letterSpacing: '0.02em' }],
        xs: ['11px', { lineHeight: '15px' }],
        sm: ['11.5px', { lineHeight: '16px' }],
        base: ['12.5px', { lineHeight: '18px' }],
        md: ['13.5px', { lineHeight: '19px' }],
        lg: ['15px', { lineHeight: '21px' }],
        xl: ['17.5px', { lineHeight: '24px' }],
        '2xl': ['21px', { lineHeight: '28px' }],
      },
      boxShadow: {
        hair: '0 0 0 1px hsl(var(--c-shadow) / 0.06)',
        rest: '0 1px 2px hsl(var(--c-shadow) / 0.05), 0 0 0 1px hsl(var(--c-shadow) / 0.045)',
        lift: '0 6px 16px -6px hsl(var(--c-shadow) / 0.18), 0 2px 5px -2px hsl(var(--c-shadow) / 0.08), 0 0 0 1px hsl(var(--c-shadow) / 0.05)',
        pop: '0 18px 44px -12px hsl(var(--c-shadow) / 0.30), 0 4px 12px -4px hsl(var(--c-shadow) / 0.12), 0 0 0 1px hsl(var(--c-shadow) / 0.06)',
        dock: '0 -10px 28px -18px hsl(var(--c-shadow) / 0.35)',
        accent: '0 6px 18px -8px hsl(var(--c-accent) / 0.65)',
      },
      transitionTimingFunction: {
        out: 'cubic-bezier(0.22, 0.61, 0.36, 1)',
        'in-out': 'cubic-bezier(0.65, 0, 0.35, 1)',
      },
      keyframes: {
        shimmer: {
          '0%': { transform: 'translateX(-100%)' },
          '100%': { transform: 'translateX(210%)' },
        },
        halo: {
          '0%': { transform: 'scale(1)', opacity: '0.55' },
          '70%': { transform: 'scale(2.5)', opacity: '0' },
          '100%': { transform: 'scale(2.5)', opacity: '0' },
        },
        // dasharray is 52 against a ~50.3 circumference, so the visible arc
        // length is (52 - offset): a short tick that sweeps out to ~80% of the
        // ring and coils back, while the whole thing keeps rotating.
        arc: {
          '0%': { strokeDashoffset: '44', transform: 'rotate(0deg)' },
          '50%': { strokeDashoffset: '12', transform: 'rotate(320deg)' },
          '100%': { strokeDashoffset: '44', transform: 'rotate(1080deg)' },
        },
      },
      animation: {
        shimmer: 'shimmer 1.5s cubic-bezier(0.4, 0, 0.2, 1) infinite',
        halo: 'halo 2.2s cubic-bezier(0.22, 0.61, 0.36, 1) infinite',
        arc: 'arc 1.6s cubic-bezier(0.5, 0.1, 0.4, 0.9) infinite',
      },
    },
  },
  plugins: [],
}

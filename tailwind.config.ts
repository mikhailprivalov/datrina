import type { Config } from 'tailwindcss';

const config: Config = {
  darkMode: 'class',
  content: ['./src/**/*.{ts,tsx}'],
  theme: {
    extend: {
      colors: {
        background: 'hsl(30 20% 98%)',
        foreground: 'hsl(30 10% 15%)',
        card: 'hsl(30 15% 99%)',
        'card-foreground': 'hsl(30 10% 15%)',
        popover: 'hsl(30 10% 99%)',
        'popover-foreground': 'hsl(30 10% 15%)',
        primary: 'hsl(25 45% 45%)',
        'primary-foreground': 'hsl(30 20% 99%)',
        secondary: 'hsl(30 15% 92%)',
        'secondary-foreground': 'hsl(30 10% 20%)',
        muted: 'hsl(30 10% 92%)',
        'muted-foreground': 'hsl(30 5% 45%)',
        accent: 'hsl(180 30% 40%)',
        'accent-foreground': 'hsl(30 20% 99%)',
        destructive: 'hsl(0 60% 50%)',
        'destructive-foreground': 'hsl(0 0% 99%)',
        border: 'hsl(30 15% 88%)',
        input: 'hsl(30 15% 88%)',
        ring: 'hsl(25 45% 45%)',
        chart: {
          1: 'hsl(25 50% 50%)',
          2: 'hsl(180 35% 42%)',
          3: 'hsl(45 40% 55%)',
          4: 'hsl(340 30% 50%)',
          5: 'hsl(160 30% 45%)',
          6: 'hsl(15 45% 55%)',
        },
      },
      borderRadius: {
        lg: '0.625rem',
        md: '0.5rem',
        sm: '0.375rem',
      },
      fontFamily: {
        sans: ['Inter', 'system-ui', 'sans-serif'],
        mono: ['JetBrains Mono', 'monospace'],
      },
    },
  },
  plugins: [],
};

export default config;

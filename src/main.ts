import { createApp } from 'vue';
import App from './App.vue';
import { applyTheme, DEFAULT_THEME } from './theme';
import './style.css';
import './theme.css';
import './motion.css';

applyTheme(DEFAULT_THEME);
createApp(App).mount('#app');

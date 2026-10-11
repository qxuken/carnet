import { mount } from 'svelte';
import './styles.css';
import App from './App.svelte';
import { sync } from './lib/sync.svelte';

// For timing and poking at it in development (`window.sync`); not in a build.
if (import.meta.env.DEV) (window as unknown as { sync: typeof sync }).sync = sync;

export default mount(App, { target: document.getElementById('app')! });

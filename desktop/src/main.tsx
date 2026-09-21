import React from 'react';
import ReactDOM from 'react-dom/client';
import { MantineProvider, createTheme } from '@mantine/core';
import '@mantine/core/styles.css';
import './style.css';
import { App } from './App';

const theme = createTheme({
  fontFamily: 'Arial, sans-serif',
  fontSizes: { xs: '0.8rem', sm: '1rem', md: '1rem', lg: '1.25rem', xl: '1.25rem' },
  headings: { fontFamily: 'Arial, sans-serif', fontWeight: '600' },
  primaryColor: 'blue',
});

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode><MantineProvider theme={theme} defaultColorScheme="dark"><App /></MantineProvider></React.StrictMode>,
);

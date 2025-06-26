import React, { useState, useCallback, useRef, useEffect } from 'react';
import { SearchAddon } from '@xterm/addon-search';
import { Search, X, ChevronUp, ChevronDown } from 'lucide-react';
import { Button } from '../ui/Button';
import { sanitizeSearchQuery } from '../../utils/security';

interface TerminalSearchProps {
  onSearch: (query: string, options?: { caseSensitive?: boolean; wholeWord?: boolean; regex?: boolean }) => void;
  onClose: () => void;
  searchAddon: SearchAddon | null;
}

export const TerminalSearch: React.FC<TerminalSearchProps> = ({
  onSearch,
  onClose,
  searchAddon
}) => {
  const [query, setQuery] = useState('');
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [wholeWord, setWholeWord] = useState(false);
  const [regex, setRegex] = useState(false);
  const [matchCount, setMatchCount] = useState(0);
  const [currentMatch, setCurrentMatch] = useState(0);
  
  const inputRef = useRef<HTMLInputElement>(null);

  // Focus input when component mounts
  useEffect(() => {
    if (inputRef.current) {
      inputRef.current.focus();
    }
  }, []);

  const handleSearch = useCallback((direction: 'next' | 'previous' = 'next') => {
    if (!query.trim() || !searchAddon) return;

    const options = {
      caseSensitive,
      wholeWord,
      regex
    };

    try {
      if (direction === 'next') {
        const found = searchAddon.findNext(query, options);
        if (found) {
          setCurrentMatch(prev => prev + 1);
        }
      } else {
        const found = searchAddon.findPrevious(query, options);
        if (found) {
          setCurrentMatch(prev => Math.max(1, prev - 1));
        }
      }
      
      onSearch(query, options);
    } catch (error) {
      console.error('Search error:', error);
    }
  }, [query, caseSensitive, wholeWord, regex, searchAddon, onSearch]);

  const handleKeyDown = useCallback((e: React.KeyboardEvent) => {
    switch (e.key) {
      case 'Enter':
        e.preventDefault();
        handleSearch(e.shiftKey ? 'previous' : 'next');
        break;
      case 'Escape':
        e.preventDefault();
        onClose();
        break;
      case 'F3':
        e.preventDefault();
        handleSearch(e.shiftKey ? 'previous' : 'next');
        break;
    }
  }, [handleSearch, onClose]);

  const handleInputChange = useCallback((e: React.ChangeEvent<HTMLInputElement>) => {
    const rawQuery = e.target.value;
    const sanitizedQuery = sanitizeSearchQuery(rawQuery);
    
    // If sanitization changed the query, warn the user
    if (rawQuery !== sanitizedQuery) {
      console.warn('Search query was sanitized for security');
    }
    
    setQuery(sanitizedQuery);
    setCurrentMatch(0);
    setMatchCount(0);
    
    if (sanitizedQuery.trim() && searchAddon) {
      // Start fresh search
      try {
        const found = searchAddon.findNext(sanitizedQuery, { caseSensitive, wholeWord, regex });
        if (found) {
          setCurrentMatch(1);
        }
      } catch (error) {
        console.error('Search error:', error);
      }
    }
  }, [searchAddon, caseSensitive, wholeWord, regex]);

  return (
    <div className="terminal-search bg-background border-b border-border p-2 flex items-center space-x-2">
      <div className="relative flex-1">
        <Search className="absolute left-2 top-1/2 transform -translate-y-1/2 w-4 h-4 text-muted-foreground" />
        <input
          ref={inputRef}
          type="text"
          value={query}
          onChange={handleInputChange}
          onKeyDown={handleKeyDown}
          placeholder="Search terminal..."
          className="w-full pl-8 pr-4 py-1 text-sm border rounded bg-background text-foreground placeholder-muted-foreground focus:outline-none focus:ring-2 focus:ring-ring"
        />
      </div>
      
      <div className="flex items-center space-x-1">
        <Button
          variant="ghost"
          size="sm"
          onClick={() => handleSearch('previous')}
          disabled={!query.trim()}
          title="Previous match (Shift+Enter)"
        >
          <ChevronUp className="w-4 h-4" />
        </Button>
        
        <Button
          variant="ghost"
          size="sm"
          onClick={() => handleSearch('next')}
          disabled={!query.trim()}
          title="Next match (Enter)"
        >
          <ChevronDown className="w-4 h-4" />
        </Button>
      </div>
      
      <div className="flex items-center space-x-2">
        <label className="flex items-center space-x-1 text-sm">
          <input
            type="checkbox"
            checked={caseSensitive}
            onChange={(e) => setCaseSensitive(e.target.checked)}
            className="rounded"
          />
          <span>Aa</span>
        </label>
        
        <label className="flex items-center space-x-1 text-sm">
          <input
            type="checkbox"
            checked={wholeWord}
            onChange={(e) => setWholeWord(e.target.checked)}
            className="rounded"
          />
          <span>Ab</span>
        </label>
        
        <label className="flex items-center space-x-1 text-sm">
          <input
            type="checkbox"
            checked={regex}
            onChange={(e) => setRegex(e.target.checked)}
            className="rounded"
          />
          <span>.*</span>
        </label>
      </div>
      
      {query && matchCount > 0 && (
        <div className="text-sm text-muted-foreground">
          {currentMatch} of {matchCount}
        </div>
      )}
      
      <Button
        variant="ghost"
        size="sm"
        onClick={onClose}
        title="Close search (Escape)"
      >
        <X className="w-4 h-4" />
      </Button>
    </div>
  );
};
import React, { useEffect, useRef } from 'react';
import { Copy, Clipboard, MousePointer } from 'lucide-react';

interface TerminalContextMenuProps {
  x: number;
  y: number;
  onCopy: () => void;
  onPaste: () => void;
  onSelectAll: () => void;
  onClose: () => void;
  hasSelection: boolean;
}

export const TerminalContextMenu: React.FC<TerminalContextMenuProps> = ({
  x,
  y,
  onCopy,
  onPaste,
  onSelectAll,
  onClose,
  hasSelection
}) => {
  const menuRef = useRef<HTMLDivElement>(null);

  // Position menu and handle outside clicks
  useEffect(() => {
    const menu = menuRef.current;
    if (!menu) return;

    // Position the menu
    const rect = menu.getBoundingClientRect();
    const viewportWidth = window.innerWidth;
    const viewportHeight = window.innerHeight;
    
    // Adjust position if menu would go off screen
    let adjustedX = x;
    let adjustedY = y;
    
    if (x + rect.width > viewportWidth) {
      adjustedX = viewportWidth - rect.width - 10;
    }
    
    if (y + rect.height > viewportHeight) {
      adjustedY = viewportHeight - rect.height - 10;
    }
    
    menu.style.left = `${adjustedX}px`;
    menu.style.top = `${adjustedY}px`;

    // Handle clicks outside menu
    const handleClickOutside = (event: MouseEvent) => {
      if (!menu.contains(event.target as Node)) {
        onClose();
      }
    };

    // Small delay to prevent immediate closure
    setTimeout(() => {
      document.addEventListener('click', handleClickOutside);
    }, 10);

    return () => {
      document.removeEventListener('click', handleClickOutside);
    };
  }, [x, y, onClose]);

  const handleMenuAction = (action: () => void) => {
    action();
    onClose();
  };

  return (
    <div
      ref={menuRef}
      className="fixed z-50 bg-popover text-popover-foreground border border-border rounded-md shadow-lg py-1 min-w-[140px]"
      style={{
        left: x,
        top: y
      }}
    >
      <button
        onClick={() => handleMenuAction(onCopy)}
        disabled={!hasSelection}
        className="w-full flex items-center space-x-2 px-3 py-1.5 text-sm hover:bg-accent hover:text-accent-foreground disabled:opacity-50 disabled:cursor-not-allowed"
      >
        <Copy className="w-4 h-4" />
        <span>Copy</span>
      </button>
      
      <button
        onClick={() => handleMenuAction(onPaste)}
        className="w-full flex items-center space-x-2 px-3 py-1.5 text-sm hover:bg-accent hover:text-accent-foreground"
      >
        <Clipboard className="w-4 h-4" />
        <span>Paste</span>
      </button>
      
      <div className="border-t border-border my-1" />
      
      <button
        onClick={() => handleMenuAction(onSelectAll)}
        className="w-full flex items-center space-x-2 px-3 py-1.5 text-sm hover:bg-accent hover:text-accent-foreground"
      >
        <MousePointer className="w-4 h-4" />
        <span>Select All</span>
      </button>
    </div>
  );
};